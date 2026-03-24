use crate::model::{
    AppError, AppResult, DownloadEntry, DownloadPolicySettings, DownloadSavePolicy,
    DownloadSitePolicy, DownloadState,
};
use reqwest::blocking::Client;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    ETAG, LAST_MODIFIED, RANGE,
};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(60 * 15);
const CHUNK_SIZE: usize = 16 * 1024;

#[derive(Debug)]
pub struct DownloadManager {
    next_id: u64,
    downloads: BTreeMap<u64, DownloadHandle>,
    default_policy: DownloadSavePolicy,
    site_policies: BTreeMap<String, DownloadSavePolicy>,
}

#[derive(Debug)]
struct DownloadHandle {
    shared: Arc<Mutex<DownloadShared>>,
    pause_flag: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct DownloadShared {
    entry: DownloadEntry,
    temp_path: String,
    resume_validator: ResumeValidator,
}

#[derive(Debug, Clone, Default)]
struct ResumeValidator {
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkerConfig {
    id: u64,
    url: String,
    destination_path: PathBuf,
    temp_path: PathBuf,
}

impl DownloadManager {
    pub fn enqueue(&mut self, url: &str) -> AppResult<DownloadEntry> {
        let parsed = Url::parse(url)
            .map_err(|error| AppError::validation(format!("Invalid download URL: {error}")))?;
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(AppError::validation(format!(
                    "Unsupported download scheme: {scheme}"
                )));
            }
        }

        self.next_id += 1;
        let id = self.next_id;
        let save_policy = self.resolve_save_policy(&parsed);
        let base_name = default_filename_from_url(&parsed);
        let destination_path =
            unique_destination_path(Path::new(&save_policy.directory), &base_name);
        let temp_path = temp_download_path(&destination_path, id);
        let now = unix_timestamp_ms();

        let entry = DownloadEntry {
            id,
            url: url.to_string(),
            final_url: None,
            file_name: destination_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin")
                .to_string(),
            save_path: destination_path.display().to_string(),
            total_bytes: None,
            downloaded_bytes: 0,
            state: DownloadState::Queued,
            supports_resume: None,
            save_policy,
            last_error: None,
            status_message: Some("Queued download request".to_string()),
            created_at_ms: now,
            updated_at_ms: now,
        };

        let shared = Arc::new(Mutex::new(DownloadShared {
            entry: entry.clone(),
            temp_path: temp_path.display().to_string(),
            resume_validator: ResumeValidator::default(),
        }));
        let handle = DownloadHandle {
            shared: Arc::clone(&shared),
            pause_flag: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        };
        let config = WorkerConfig {
            id,
            url: url.to_string(),
            destination_path,
            temp_path,
        };
        self.downloads.insert(id, handle);
        self.spawn_worker(id, config, false)?;
        self.download(id)
    }

    pub fn get_policy_settings(&self) -> DownloadPolicySettings {
        DownloadPolicySettings {
            default_policy: self.default_policy.clone(),
            site_policies: self
                .site_policies
                .iter()
                .map(|(origin, policy)| DownloadSitePolicy {
                    origin: origin.clone(),
                    policy: policy.clone(),
                })
                .collect(),
        }
    }

    pub fn set_default_policy(
        &mut self,
        policy: DownloadSavePolicy,
    ) -> AppResult<DownloadPolicySettings> {
        self.default_policy = normalize_save_policy(policy)?;
        Ok(self.get_policy_settings())
    }

    pub fn set_site_policy(
        &mut self,
        origin: &str,
        policy: DownloadSavePolicy,
    ) -> AppResult<DownloadPolicySettings> {
        let canonical_origin = canonicalize_origin(origin)?;
        self.site_policies
            .insert(canonical_origin, normalize_save_policy(policy)?);
        Ok(self.get_policy_settings())
    }

    pub fn clear_site_policy(&mut self, origin: &str) -> AppResult<DownloadPolicySettings> {
        let canonical_origin = canonicalize_origin(origin)?;
        self.site_policies.remove(&canonical_origin);
        Ok(self.get_policy_settings())
    }

    pub fn apply_policy_settings(&mut self, settings: DownloadPolicySettings) -> AppResult<()> {
        self.default_policy = normalize_save_policy(settings.default_policy)?;
        self.site_policies.clear();
        for entry in settings.site_policies {
            let canonical_origin = canonicalize_origin(&entry.origin)?;
            self.site_policies
                .insert(canonical_origin, normalize_save_policy(entry.policy)?);
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<DownloadEntry> {
        self.downloads
            .values()
            .filter_map(|handle| handle.shared.lock().ok().map(|shared| shared.entry.clone()))
            .collect()
    }

    pub fn progress(&self, id: u64) -> AppResult<DownloadEntry> {
        self.download(id)
    }

    pub fn pause(&mut self, id: u64) -> AppResult<DownloadEntry> {
        let handle = self
            .downloads
            .get(&id)
            .ok_or_else(|| AppError::state(format!("Download {id} does not exist")))?;
        handle.pause_flag.store(true, Ordering::SeqCst);
        update_entry(&handle.shared, |entry| {
            if matches!(
                entry.state,
                DownloadState::Downloading | DownloadState::Queued
            ) {
                // Implementation note: pausing a streaming download is cooperative.
                // We only transition to `Paused` once the worker reaches its next
                // chunk boundary, closes the response body, and leaves the partial
                // file in a resumable state. Until then the command is treated as a
                // pause request rather than an already-completed pause.
                entry.status_message =
                    Some("Pause requested; stopping after current chunk".to_string());
                entry.updated_at_ms = unix_timestamp_ms();
            }
        })?;
        self.download(id)
    }

    pub fn resume(&mut self, id: u64) -> AppResult<DownloadEntry> {
        let handle = self
            .downloads
            .get(&id)
            .ok_or_else(|| AppError::state(format!("Download {id} does not exist")))?;
        let entry = handle
            .shared
            .lock()
            .map_err(|_| AppError::state("Download state lock poisoned"))?
            .entry
            .clone();
        if !matches!(entry.state, DownloadState::Paused | DownloadState::Failed) {
            return Err(AppError::state(format!(
                "Download {id} is not paused or failed"
            )));
        }
        handle.pause_flag.store(false, Ordering::SeqCst);
        handle.cancel_flag.store(false, Ordering::SeqCst);
        let config = WorkerConfig {
            id,
            url: entry.url,
            destination_path: PathBuf::from(&entry.save_path),
            temp_path: PathBuf::from(temp_download_path(Path::new(&entry.save_path), id)),
        };
        self.spawn_worker(id, config, true)?;
        self.download(id)
    }

    pub fn cancel(&mut self, id: u64) -> AppResult<DownloadEntry> {
        let handle = self
            .downloads
            .get(&id)
            .ok_or_else(|| AppError::state(format!("Download {id} does not exist")))?;
        handle.cancel_flag.store(true, Ordering::SeqCst);
        handle.pause_flag.store(false, Ordering::SeqCst);
        update_entry(&handle.shared, |entry| {
            entry.state = DownloadState::Cancelled;
            entry.status_message = Some("Cancel requested".to_string());
            entry.updated_at_ms = unix_timestamp_ms();
        })?;
        self.download(id)
    }

    pub fn open(&self, id: u64) -> AppResult<DownloadEntry> {
        let entry = self.download(id)?;
        if entry.state != DownloadState::Completed {
            return Err(AppError::state(format!(
                "Download {id} is not completed yet"
            )));
        }
        if !Path::new(&entry.save_path).exists() {
            return Err(AppError::download_save_failed(format!(
                "Downloaded file is missing: {}",
                entry.save_path
            )));
        }
        Ok(entry)
    }

    pub fn reveal(&self, id: u64) -> AppResult<DownloadEntry> {
        self.open(id)
    }

    fn download(&self, id: u64) -> AppResult<DownloadEntry> {
        self.downloads
            .get(&id)
            .ok_or_else(|| AppError::state(format!("Download {id} does not exist")))?
            .shared
            .lock()
            .map_err(|_| AppError::state("Download state lock poisoned"))
            .map(|shared| shared.entry.clone())
    }

    fn spawn_worker(&self, id: u64, config: WorkerConfig, is_resume: bool) -> AppResult<()> {
        let handle = self
            .downloads
            .get(&id)
            .ok_or_else(|| AppError::state(format!("Download {id} does not exist")))?;
        let shared = Arc::clone(&handle.shared);
        let pause_flag = Arc::clone(&handle.pause_flag);
        let cancel_flag = Arc::clone(&handle.cancel_flag);
        thread::Builder::new()
            .name(format!("cosmobrowse-download-{id}"))
            .spawn(move || {
                run_download_worker(shared, pause_flag, cancel_flag, config, is_resume);
            })
            .map_err(|error| {
                AppError::state(format!("Failed to spawn download worker: {error}"))
            })?;
        Ok(())
    }

    fn resolve_save_policy(&self, url: &Url) -> DownloadSavePolicy {
        let origin = origin_key_from_url(url);
        self.site_policies
            .get(&origin)
            .cloned()
            .unwrap_or_else(|| self.default_policy.clone())
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self {
            next_id: 0,
            downloads: BTreeMap::new(),
            default_policy: default_save_policy(),
            site_policies: BTreeMap::new(),
        }
    }
}

fn run_download_worker(
    shared: Arc<Mutex<DownloadShared>>,
    pause_flag: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
    config: WorkerConfig,
    is_resume: bool,
) {
    if let Err(error) =
        execute_download_worker(&shared, &pause_flag, &cancel_flag, &config, is_resume)
    {
        let _ = update_entry(&shared, |entry| {
            if entry.state != DownloadState::Cancelled {
                entry.state = DownloadState::Failed;
            }
            entry.last_error = Some(error.clone());
            entry.status_message = Some(error.message.clone());
            entry.updated_at_ms = unix_timestamp_ms();
        });
    }
}

fn execute_download_worker(
    shared: &Arc<Mutex<DownloadShared>>,
    pause_flag: &Arc<AtomicBool>,
    cancel_flag: &Arc<AtomicBool>,
    config: &WorkerConfig,
    is_resume: bool,
) -> AppResult<()> {
    if let Some(parent) = config.destination_path.parent() {
        fs::create_dir_all(parent).map_err(classify_fs_error)?;
    }

    let mut resume_from = fs::metadata(&config.temp_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if !is_resume && resume_from > 0 {
        fs::remove_file(&config.temp_path).map_err(classify_fs_error)?;
        resume_from = 0;
    }

    update_entry(shared, |entry| {
        entry.state = DownloadState::Downloading;
        entry.last_error = None;
        entry.status_message = Some(if is_resume {
            "Resuming download".to_string()
        } else {
            "Starting download".to_string()
        });
        entry.updated_at_ms = unix_timestamp_ms();
    })?;

    let client = download_http_client(&config.url)?;
    let mut request = client.get(&config.url);

    // Spec note: resumable retrieval uses RFC 9110 Range requests by asking for
    // `bytes=<start>-`. When a server answers with 206 Partial Content we append
    // bytes to the existing `.part` file; when it ignores the range and returns
    // 200 OK we restart from byte 0, matching the fallback policy requested by
    // product and staying consistent with RFC 9110 range semantics.
    // https://www.rfc-editor.org/rfc/rfc9110.html#name-range
    // https://www.rfc-editor.org/rfc/rfc9110.html#name-partial-content
    if resume_from > 0 {
        request = request.header(RANGE, format!("bytes={resume_from}-"));
    }

    let mut response = request
        .send()
        .map_err(|error| AppError::network(format!("Failed to start download: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::network(format!(
            "Download request failed with HTTP status {status}"
        )));
    }

    let headers = response.headers().clone();
    let final_url = response.url().to_string();
    let (resume_from, supports_resume) =
        apply_resume_response_policy(shared, config, resume_from, status, &headers)?;
    let response_validator = ResumeValidator::from_headers(&headers);
    set_resume_validator(shared, response_validator)?;

    let resolved_name = suggested_filename(&headers, &final_url).unwrap_or_else(|| {
        config
            .destination_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("download.bin")
            .to_string()
    });

    let destination_path = unique_destination_path(
        config
            .destination_path
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        &resolved_name,
    );
    let temp_path = temp_download_path(&destination_path, config.id);

    let mut file = if resume_from > 0 {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&temp_path)
            .map_err(classify_fs_error)?
    } else {
        File::create(&temp_path).map_err(classify_fs_error)?
    };

    let total_bytes = total_bytes_from_headers(&headers, resume_from);
    update_entry(shared, |entry| {
        entry.final_url = Some(final_url.clone());
        entry.file_name = destination_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("download.bin")
            .to_string();
        entry.save_path = destination_path.display().to_string();
        entry.total_bytes = total_bytes;
        entry.downloaded_bytes = resume_from;
        entry.supports_resume = supports_resume;
        entry.updated_at_ms = unix_timestamp_ms();
    })?;

    let mut buffer = [0u8; CHUNK_SIZE];
    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&temp_path);
            update_entry(shared, |entry| {
                entry.state = DownloadState::Cancelled;
                entry.status_message = Some("Download cancelled".to_string());
                entry.updated_at_ms = unix_timestamp_ms();
            })?;
            return Ok(());
        }
        if pause_flag.load(Ordering::SeqCst) {
            update_entry(shared, |entry| {
                entry.state = DownloadState::Paused;
                entry.status_message = Some("Download paused".to_string());
                entry.updated_at_ms = unix_timestamp_ms();
            })?;
            return Ok(());
        }

        let read = response.read(&mut buffer).map_err(|error| {
            AppError::network(format!("Failed while downloading body: {error}"))
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(classify_fs_error)?;
        file.flush().map_err(classify_fs_error)?;
        update_entry(shared, |entry| {
            entry.downloaded_bytes += read as u64;
            entry.status_message = Some(format!("Downloaded {} bytes", entry.downloaded_bytes));
            entry.updated_at_ms = unix_timestamp_ms();
        })?;
    }

    fs::rename(&temp_path, &destination_path).map_err(classify_fs_error)?;
    update_entry(shared, |entry| {
        entry.state = DownloadState::Completed;
        entry.downloaded_bytes = entry.total_bytes.unwrap_or(entry.downloaded_bytes);
        entry.save_path = destination_path.display().to_string();
        entry.status_message = Some("Download completed".to_string());
        entry.updated_at_ms = unix_timestamp_ms();
    })?;
    Ok(())
}

fn update_entry(
    shared: &Arc<Mutex<DownloadShared>>,
    update: impl FnOnce(&mut DownloadEntry),
) -> AppResult<()> {
    let mut shared = shared
        .lock()
        .map_err(|_| AppError::state("Download state lock poisoned"))?;
    update(&mut shared.entry);
    shared.temp_path = temp_download_path(Path::new(&shared.entry.save_path), shared.entry.id)
        .display()
        .to_string();
    Ok(())
}

fn apply_resume_response_policy(
    shared: &Arc<Mutex<DownloadShared>>,
    config: &WorkerConfig,
    mut resume_from: u64,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
) -> AppResult<(u64, Option<bool>)> {
    let mut supports_resume = headers
        .get(ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("bytes"));

    // Spec note: resumable retrieval asks for `bytes=<start>-`, but RFC 9110
    // allows an origin to ignore Range and send the full selected
    // representation with `200 OK`. When that happens after a partial file
    // already exists, we must truncate the `.part` file and restart from byte 0
    // instead of appending a full-body 200 response to stale partial bytes.
    // https://www.rfc-editor.org/rfc/rfc9110.html#name-range
    // https://www.rfc-editor.org/rfc/rfc9110.html#name-ok-200
    // https://www.rfc-editor.org/rfc/rfc9110.html#name-partial-content
    if resume_from > 0 && status.as_u16() != 206 {
        supports_resume = Some(false);
        resume_from = 0;
        File::create(&config.temp_path).map_err(classify_fs_error)?;
        update_entry(shared, |entry| {
            entry.downloaded_bytes = 0;
            entry.last_error = Some(AppError::download_resume_unsupported(
                "Server did not honor the Range request; restarted download from byte 0",
            ));
            entry.status_message = Some(
                "Server ignored resume request; restarting download from the beginning".to_string(),
            );
            entry.supports_resume = Some(false);
            entry.updated_at_ms = unix_timestamp_ms();
        })?;
    }

    if resume_from > 0 && status.as_u16() == 206 {
        // Spec note: RFC 9110 requires a valid `Content-Range` field in `206
        // Partial Content` responses. We reject malformed/offset-mismatched
        // ranges and restart from byte 0 to avoid appending bytes from a
        // divergent representation to an existing `.part` file.
        // https://www.rfc-editor.org/rfc/rfc9110.html#name-partial-content
        // https://www.rfc-editor.org/rfc/rfc9110.html#name-content-range
        if !content_range_matches_resume_offset(headers, resume_from) {
            supports_resume = Some(false);
            resume_from = 0;
            File::create(&config.temp_path).map_err(classify_fs_error)?;
            update_entry(shared, |entry| {
                entry.downloaded_bytes = 0;
                entry.last_error = Some(AppError::download_resume_unsupported(
                    "Server returned an invalid Content-Range for resume; restarted from byte 0",
                ));
                entry.status_message = Some(
                    "Resume response had invalid Content-Range; restarting download".to_string(),
                );
                entry.supports_resume = Some(false);
                entry.updated_at_ms = unix_timestamp_ms();
            })?;
        } else if resume_validator_mismatch(shared, headers)? {
            // Spec note: RFC 9111 validator rules require cached partial bytes to
            // be reused only when validators still identify the same selected
            // representation. If ETag/Last-Modified no longer match, we restart
            // full-body download to preserve representation integrity.
            // https://www.rfc-editor.org/rfc/rfc9111.html#section-4.3
            supports_resume = Some(false);
            resume_from = 0;
            File::create(&config.temp_path).map_err(classify_fs_error)?;
            update_entry(shared, |entry| {
                entry.downloaded_bytes = 0;
                entry.last_error = Some(AppError::download_resume_unsupported(
                    "Stored validators did not match resume response; restarted from byte 0",
                ));
                entry.status_message = Some(
                    "Resume validator mismatch (ETag/Last-Modified); restarting download"
                        .to_string(),
                );
                entry.supports_resume = Some(false);
                entry.updated_at_ms = unix_timestamp_ms();
            })?;
        }
    }

    Ok((resume_from, supports_resume))
}

fn content_range_matches_resume_offset(headers: &HeaderMap, resume_from: u64) -> bool {
    let Some(content_range) = headers.get(CONTENT_RANGE).and_then(header_value_to_string) else {
        return false;
    };
    let Some(range_spec) = content_range.strip_prefix("bytes ") else {
        return false;
    };
    let Some((range_part, _total_part)) = range_spec.split_once('/') else {
        return false;
    };
    let Some((start_part, _end_part)) = range_part.split_once('-') else {
        return false;
    };
    start_part.parse::<u64>().ok() == Some(resume_from)
}

fn resume_validator_mismatch(
    shared: &Arc<Mutex<DownloadShared>>,
    headers: &HeaderMap,
) -> AppResult<bool> {
    let stored = shared
        .lock()
        .map_err(|_| AppError::state("Download state lock poisoned"))?
        .resume_validator
        .clone();
    Ok(stored.mismatches(headers))
}

fn set_resume_validator(
    shared: &Arc<Mutex<DownloadShared>>,
    validator: ResumeValidator,
) -> AppResult<()> {
    let mut shared = shared
        .lock()
        .map_err(|_| AppError::state("Download state lock poisoned"))?;
    shared.resume_validator = validator;
    Ok(())
}

fn header_value_to_string(value: &HeaderValue) -> Option<String> {
    value.to_str().ok().map(str::to_string)
}

impl ResumeValidator {
    fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            etag: headers.get(ETAG).and_then(header_value_to_string),
            last_modified: headers.get(LAST_MODIFIED).and_then(header_value_to_string),
        }
    }

    fn mismatches(&self, headers: &HeaderMap) -> bool {
        if self.etag.is_none() && self.last_modified.is_none() {
            return false;
        }
        if let Some(expected_etag) = &self.etag {
            let Some(actual_etag) = headers.get(ETAG).and_then(header_value_to_string) else {
                return true;
            };
            if &actual_etag != expected_etag {
                return true;
            }
        }
        if let Some(expected_last_modified) = &self.last_modified {
            let Some(actual_last_modified) =
                headers.get(LAST_MODIFIED).and_then(header_value_to_string)
            else {
                return true;
            };
            if &actual_last_modified != expected_last_modified {
                return true;
            }
        }
        false
    }
}

fn total_bytes_from_headers(headers: &HeaderMap, resume_from: u64) -> Option<u64> {
    if let Some(content_range) = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
    {
        if let Some((_, total)) = content_range.rsplit_once('/') {
            if let Ok(total) = total.parse::<u64>() {
                return Some(total);
            }
        }
    }
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value + resume_from)
}

fn suggested_filename(headers: &HeaderMap, final_url: &str) -> Option<String> {
    // Spec note: attachment handling prefers `filename*=` then `filename=` from
    // Content-Disposition, following RFC 9110 field processing for representation
    // metadata. We intentionally require explicit user action before writing to
    // disk instead of auto-downloading during navigation, which aligns with Fetch /
    // HTML download UX where downloads are user-mediated rather than silently
    // rendered in the browsing context.
    // https://www.rfc-editor.org/rfc/rfc9110.html#field.content-disposition
    // https://html.spec.whatwg.org/multipage/links.html#downloading-resources
    let disposition = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?;
    for part in disposition.split(';').map(str::trim) {
        if let Some(value) = part.strip_prefix("filename*=UTF-8''") {
            return Some(sanitize_filename(&percent_decode(value)));
        }
    }
    for part in disposition.split(';').map(str::trim) {
        if let Some(value) = part.strip_prefix("filename=") {
            return Some(sanitize_filename(value.trim_matches('"')));
        }
    }
    Url::parse(final_url)
        .ok()
        .map(|url| default_filename_from_url(&url))
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                bytes.push(decoded);
                index += 3;
                continue;
            }
        }
        bytes.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn default_filename_from_url(url: &Url) -> String {
    let candidate = url
        .path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).next_back())
        .filter(|segment| !segment.is_empty())
        .unwrap_or("download.bin");
    sanitize_filename(candidate)
}

fn sanitize_filename(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string();
    if sanitized.is_empty() {
        "download.bin".to_string()
    } else {
        sanitized
    }
}

fn unique_destination_path(directory: &Path, file_name: &str) -> PathBuf {
    let base = sanitize_filename(file_name);
    let stem = Path::new(&base)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let ext = Path::new(&base)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let mut candidate = directory.join(&base);
    let mut counter = 1u64;
    while candidate.exists() {
        candidate = directory.join(format!("{stem} ({counter}){ext}"));
        counter += 1;
    }
    candidate
}

fn temp_download_path(destination_path: &Path, id: u64) -> PathBuf {
    destination_path.with_extension(format!("{}.part", id))
}

fn default_save_policy() -> DownloadSavePolicy {
    let directory = std::env::var("COSMO_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home_downloads = std::env::var("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Downloads"));
            match home_downloads {
                Ok(path) if path.exists() => path,
                _ => std::env::temp_dir().join("cosmobrowse-downloads"),
            }
        });
    DownloadSavePolicy {
        directory: directory.display().to_string(),
        conflict_policy: "uniquify".to_string(),
        requires_user_confirmation: true,
    }
}

fn normalize_save_policy(mut policy: DownloadSavePolicy) -> AppResult<DownloadSavePolicy> {
    // Spec note: UI file pickers may return leading/trailing whitespace and
    // non-canonical path strings. We normalize and validate once at the adapter
    // boundary so downstream file operations are deterministic and least-surprise.
    let trimmed = policy.directory.trim();
    if trimmed.is_empty() {
        return Err(AppError::validation(
            "Download save directory must not be empty",
        ));
    }
    policy.directory = PathBuf::from(trimmed).display().to_string();
    if policy.conflict_policy.trim().is_empty() {
        policy.conflict_policy = "uniquify".to_string();
    }
    Ok(policy)
}

fn origin_key_from_url(url: &Url) -> String {
    if let Some(port) = url.port() {
        return format!("{}://{}:{port}", url.scheme(), url.host_str().unwrap_or_default());
    }
    format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default())
}

fn canonicalize_origin(origin: &str) -> AppResult<String> {
    // Spec note: Origin identity follows RFC 6454 tuple semantics
    // (scheme/host/port). We parse and normalize the tuple so policy keys match
    // exactly across commands and session restarts.
    // https://www.rfc-editor.org/rfc/rfc6454#section-4
    let parsed = Url::parse(origin)
        .map_err(|error| AppError::validation(format!("Invalid origin URL: {error}")))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(AppError::validation(format!(
            "Download site policy origin must be http/https: {origin}"
        )));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AppError::validation(format!(
            "Download site policy origin must not include path/query/fragment: {origin}"
        )));
    }
    Ok(origin_key_from_url(&parsed))
}

fn should_bypass_proxy(url: &str) -> bool {
    let parsed = match Url::parse(url) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    let host = match parsed.host_str() {
        Some(host) => host,
        None => return false,
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn download_http_client(url: &str) -> AppResult<Client> {
    let mut builder = Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(10));
    if should_bypass_proxy(url) {
        // Test/dev note: loopback downloads should connect directly instead of
        // being routed through ambient HTTP proxies. Our download fixtures use
        // `localhost` / loopback origins, and proxying those requests can yield
        // synthetic 4xx responses before the request ever reaches the local
        // server. Bypassing proxies for loopback keeps local download behavior
        // aligned with the direct origin that generated the URL.
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| AppError::state(format!("Failed to build download HTTP client: {error}")))
}

fn classify_fs_error(error: std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return AppError::download_permission_denied(format!(
            "Download could not be saved because permission was denied: {error}"
        ));
    }
    AppError::download_save_failed(format!("Download file operation failed: {error}"))
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::sync::Once;
    use std::thread::JoinHandle;

    fn configure_loopback_proxy_bypass() {
        static CONFIGURE_PROXY_BYPASS: Once = Once::new();
        CONFIGURE_PROXY_BYPASS.call_once(|| {
            // Test-fixture note: some CI/container environments inject ambient
            // HTTP proxy variables that are consulted before the request reaches
            // our loopback fixture. Explicitly mark the standard loopback hosts
            // as non-proxy targets so the tests exercise direct local transport.
            std::env::set_var("NO_PROXY", "localhost,127.0.0.1,::1");
            std::env::set_var("no_proxy", "localhost,127.0.0.1,::1");
        });
    }

    fn spawn_fixture_server(
        range_enabled: bool,
        slow: bool,
        expected_connections: usize,
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let addr = listener.local_addr().expect("local addr");
        let body = vec![b'a'; 512 * 1024];
        let handle = thread::spawn(move || {
            // Test-fixture note: the client intentionally tears down the first
            // connection during pause/resume. Browsers may abort an in-flight body
            // when a download is paused, so the fixture must treat BrokenPipe /
            // ConnectionReset as expected transport cancellation rather than a test
            // failure. We also serve each accepted connection on its own thread so
            // the resumed request can proceed even while the abandoned connection is
            // still draining or being torn down by the OS.
            let mut connection_handles = Vec::new();
            for stream in listener.incoming().take(expected_connections) {
                let mut stream = stream.expect("stream");
                let body = body.clone();
                let connection_handle = thread::spawn(move || {
                    let mut buffer = [0u8; 4096];
                    let read = stream.read(&mut buffer).expect("read request");
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let range_header = request
                        .lines()
                        .find_map(|line| {
                            let lower = line.trim().to_ascii_lowercase();
                            lower.strip_prefix("range: bytes=").map(str::to_string)
                        })
                        .and_then(|line| line.split('-').next().map(str::to_string))
                        .and_then(|value| value.parse::<usize>().ok());
                    let start = if range_enabled {
                        range_header.unwrap_or(0)
                    } else {
                        0
                    };
                    let status_line = if range_enabled && range_header.is_some() {
                        "HTTP/1.1 206 Partial Content\r\n"
                    } else {
                        // Spec note: when `range_enabled` is false we intentionally
                        // ignore any incoming `Range` header and respond with `200 OK`
                        // plus the full representation. RFC 9110 allows an origin to
                        // ignore Range and serve the selected representation normally;
                        // the download manager should then restart from byte 0 rather
                        // than append incompatible data to the existing `.part` file.
                        // https://www.rfc-editor.org/rfc/rfc9110.html#name-range
                        // https://www.rfc-editor.org/rfc/rfc9110.html#name-ok-200
                        "HTTP/1.1 200 OK\r\n"
                    };
                    let payload = &body[start..];
                    let mut headers = format!(
                        "{status_line}Content-Length: {}\r\nContent-Disposition: attachment; filename=\"fixture.bin\"\r\n",
                        payload.len()
                    );
                    if range_enabled {
                        headers.push_str("Accept-Ranges: bytes\r\n");
                    }
                    if range_enabled && range_header.is_some() {
                        headers.push_str(&format!(
                            "Content-Range: bytes {}-{}/{}\r\n",
                            start,
                            body.len() - 1,
                            body.len()
                        ));
                    }
                    headers.push_str("\r\n");
                    if stream.write_all(headers.as_bytes()).is_err() {
                        return;
                    }
                    let chunk_delay_ms = if !range_enabled && range_header.is_none() {
                        20
                    } else if slow {
                        15
                    } else {
                        0
                    };
                    for chunk in payload.chunks(16 * 1024) {
                        if let Err(error) = stream.write_all(chunk) {
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::BrokenPipe
                                    | std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::UnexpectedEof
                            ) {
                                break;
                            }
                            panic!("write chunk: {error}");
                        }
                        if chunk_delay_ms > 0 {
                            thread::sleep(Duration::from_millis(chunk_delay_ms));
                        }
                    }
                    let _ = stream.flush();
                    let _ = stream.shutdown(Shutdown::Both);
                });
                connection_handles.push(connection_handle);
            }
            for connection_handle in connection_handles {
                connection_handle.join().expect("connection handle join");
            }
        });
        // Test-fixture note: return the exact loopback address we bound rather
        // than `localhost`. Some environments resolve `localhost` to `::1`
        // first, which would fail against an IPv4-only `127.0.0.1` listener
        // before the request ever reaches the fixture. The download client still
        // bypasses proxies because `127.0.0.1` is recognized as a loopback IP.
        (format!("http://{addr}/fixture.bin"), handle)
    }

    fn wait_for_state(
        manager: &DownloadManager,
        id: u64,
        expected: DownloadState,
        attempts: usize,
    ) -> Option<DownloadEntry> {
        for _ in 0..attempts {
            let current = manager.progress(id).expect("progress current");
            if current.state == expected {
                return Some(current);
            }
            thread::sleep(Duration::from_millis(25));
        }
        None
    }

    fn wait_for_terminal_state(
        manager: &DownloadManager,
        id: u64,
        attempts: usize,
    ) -> DownloadEntry {
        for _ in 0..attempts {
            let current = manager.progress(id).expect("progress current");
            if matches!(
                current.state,
                DownloadState::Completed | DownloadState::Failed
            ) {
                return current;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("download did not reach a terminal state");
    }

    fn seed_paused_download(
        manager: &mut DownloadManager,
        url: &str,
        partial_len: usize,
    ) -> DownloadEntry {
        let parsed = Url::parse(url).expect("valid URL");
        manager.next_id += 1;
        let id = manager.next_id;
        let save_policy = default_save_policy();
        let destination_path = unique_destination_path(
            Path::new(&save_policy.directory),
            &default_filename_from_url(&parsed),
        );
        std::fs::create_dir_all(
            destination_path
                .parent()
                .expect("destination path should have parent"),
        )
        .expect("create dir");
        let temp_path = temp_download_path(&destination_path, id);
        // Test note: resume fallback behavior only depends on the presence of a
        // partial `.part` file and paused in-memory state. Seeding that state
        // directly avoids timing races from trying to pause a live no-range
        // transfer before the worker drains the response body.
        std::fs::write(&temp_path, vec![b'a'; partial_len]).expect("write partial");

        let now = unix_timestamp_ms();
        let entry = DownloadEntry {
            id,
            url: url.to_string(),
            final_url: None,
            file_name: destination_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin")
                .to_string(),
            save_path: destination_path.display().to_string(),
            total_bytes: None,
            downloaded_bytes: partial_len as u64,
            state: DownloadState::Paused,
            supports_resume: None,
            save_policy,
            last_error: None,
            status_message: Some("Seeded paused download for resume test".to_string()),
            created_at_ms: now,
            updated_at_ms: now,
        };

        manager.downloads.insert(
            id,
            DownloadHandle {
                shared: Arc::new(Mutex::new(DownloadShared {
                    entry: entry.clone(),
                    temp_path: temp_path.display().to_string(),
                    resume_validator: ResumeValidator::default(),
                })),
                pause_flag: Arc::new(AtomicBool::new(false)),
                cancel_flag: Arc::new(AtomicBool::new(false)),
            },
        );
        entry
    }

    #[test]
    fn pause_and_resume_uses_range_when_server_supports_it() {
        let temp_dir =
            std::env::temp_dir().join(format!("cosmobrowse-download-test-{}", unix_timestamp_ms()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        configure_loopback_proxy_bypass();
        std::env::set_var("COSMO_DOWNLOAD_DIR", &temp_dir);
        let (url, server) = spawn_fixture_server(true, true, 2);
        let mut manager = DownloadManager::default();
        let entry = manager.enqueue(&url).expect("enqueue");
        thread::sleep(Duration::from_millis(60));
        let _ = manager.pause(entry.id).expect("pause");
        let paused = wait_for_state(&manager, entry.id, DownloadState::Paused, 40)
            .expect("pause should settle");
        assert!(paused.downloaded_bytes > 0);
        let _ = manager.resume(entry.id).expect("resume");
        let current = wait_for_terminal_state(&manager, entry.id, 240);
        assert_eq!(current.state, DownloadState::Completed);
        assert_eq!(current.supports_resume, Some(true));
        assert!(Path::new(&current.save_path).exists());
        server.join().expect("server join");
    }

    #[test]
    fn resume_falls_back_to_restart_when_server_does_not_support_range() {
        let temp_dir =
            std::env::temp_dir().join(format!("cosmobrowse-download-test-{}", unix_timestamp_ms()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        configure_loopback_proxy_bypass();
        std::env::set_var("COSMO_DOWNLOAD_DIR", &temp_dir);
        let mut manager = DownloadManager::default();
        let entry = seed_paused_download(&mut manager, "http://127.0.0.1/fixture.bin", 64 * 1024);
        let handle = manager
            .downloads
            .get(&entry.id)
            .expect("download handle should exist");
        let config = WorkerConfig {
            id: entry.id,
            url: entry.url.clone(),
            destination_path: PathBuf::from(&entry.save_path),
            temp_path: PathBuf::from(temp_download_path(Path::new(&entry.save_path), entry.id)),
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, "524288".parse().expect("content-length"));
        let (resume_from, supports_resume) = apply_resume_response_policy(
            &handle.shared,
            &config,
            entry.downloaded_bytes,
            reqwest::StatusCode::OK,
            &headers,
        )
        .expect("resume policy should succeed");

        let current = manager.progress(entry.id).expect("progress current");
        assert_eq!(resume_from, 0);
        assert_eq!(supports_resume, Some(false));
        assert_eq!(current.state, DownloadState::Paused);
        assert_eq!(current.downloaded_bytes, 0);
        assert_eq!(current.supports_resume, Some(false));
        assert!(current.last_error.is_some());
        let temp_len = std::fs::metadata(temp_download_path(Path::new(&entry.save_path), entry.id))
            .expect("temp metadata")
            .len();
        assert_eq!(temp_len, 0);
    }

    #[test]
    fn resume_falls_back_when_content_range_offset_is_invalid() {
        let temp_dir =
            std::env::temp_dir().join(format!("cosmobrowse-download-test-{}", unix_timestamp_ms()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::env::set_var("COSMO_DOWNLOAD_DIR", &temp_dir);
        let mut manager = DownloadManager::default();
        let entry = seed_paused_download(&mut manager, "http://127.0.0.1/fixture.bin", 64 * 1024);
        let handle = manager
            .downloads
            .get(&entry.id)
            .expect("download handle should exist");
        let config = WorkerConfig {
            id: entry.id,
            url: entry.url.clone(),
            destination_path: PathBuf::from(&entry.save_path),
            temp_path: PathBuf::from(temp_download_path(Path::new(&entry.save_path), entry.id)),
        };
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_RANGES, "bytes".parse().expect("accept-ranges"));
        headers.insert(
            CONTENT_RANGE,
            "bytes 1024-524287/524288"
                .parse()
                .expect("content-range"),
        );

        let (resume_from, supports_resume) = apply_resume_response_policy(
            &handle.shared,
            &config,
            entry.downloaded_bytes,
            reqwest::StatusCode::PARTIAL_CONTENT,
            &headers,
        )
        .expect("resume policy should succeed");

        let current = manager.progress(entry.id).expect("progress current");
        assert_eq!(resume_from, 0);
        assert_eq!(supports_resume, Some(false));
        assert_eq!(current.downloaded_bytes, 0);
        assert_eq!(current.supports_resume, Some(false));
        assert!(current
            .status_message
            .unwrap_or_default()
            .contains("Content-Range"));
    }

    #[test]
    fn resume_falls_back_when_etag_does_not_match_previous_partial() {
        let temp_dir =
            std::env::temp_dir().join(format!("cosmobrowse-download-test-{}", unix_timestamp_ms()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::env::set_var("COSMO_DOWNLOAD_DIR", &temp_dir);
        let mut manager = DownloadManager::default();
        let entry = seed_paused_download(&mut manager, "http://127.0.0.1/fixture.bin", 64 * 1024);
        let handle = manager
            .downloads
            .get(&entry.id)
            .expect("download handle should exist");
        {
            let mut shared = handle.shared.lock().expect("lock");
            shared.resume_validator = ResumeValidator {
                etag: Some("\"previous\"".to_string()),
                last_modified: None,
            };
        }
        let config = WorkerConfig {
            id: entry.id,
            url: entry.url.clone(),
            destination_path: PathBuf::from(&entry.save_path),
            temp_path: PathBuf::from(temp_download_path(Path::new(&entry.save_path), entry.id)),
        };
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_RANGES, "bytes".parse().expect("accept-ranges"));
        headers.insert(
            CONTENT_RANGE,
            format!("bytes {}-{}/{}", entry.downloaded_bytes, 524287, 524288)
                .parse()
                .expect("content-range"),
        );
        headers.insert(ETAG, "\"changed\"".parse().expect("etag"));

        let (resume_from, supports_resume) = apply_resume_response_policy(
            &handle.shared,
            &config,
            entry.downloaded_bytes,
            reqwest::StatusCode::PARTIAL_CONTENT,
            &headers,
        )
        .expect("resume policy should succeed");

        let current = manager.progress(entry.id).expect("progress current");
        assert_eq!(resume_from, 0);
        assert_eq!(supports_resume, Some(false));
        assert_eq!(current.downloaded_bytes, 0);
        assert_eq!(current.supports_resume, Some(false));
        assert!(current
            .status_message
            .unwrap_or_default()
            .contains("validator mismatch"));
    }

    #[test]
    fn site_policy_overrides_default_directory_for_matching_origin() {
        let mut manager = DownloadManager::default();
        let settings = manager
            .set_default_policy(DownloadSavePolicy {
                directory: "/tmp/default-downloads".to_string(),
                conflict_policy: "uniquify".to_string(),
                requires_user_confirmation: true,
            })
            .expect("set default policy");
        assert_eq!(settings.default_policy.directory, "/tmp/default-downloads");
        manager
            .set_site_policy(
                "https://example.com",
                DownloadSavePolicy {
                    directory: "/tmp/example-downloads".to_string(),
                    conflict_policy: "uniquify".to_string(),
                    requires_user_confirmation: false,
                },
            )
            .expect("set site policy");

        let resolved = manager.resolve_save_policy(&Url::parse("https://example.com/file.bin").expect("url"));
        assert_eq!(resolved.directory, "/tmp/example-downloads");
        assert!(!resolved.requires_user_confirmation);
    }

    #[test]
    fn canonicalize_origin_rejects_non_origin_urls() {
        let with_path = canonicalize_origin("https://example.com/downloads");
        assert!(with_path.is_err());
        let ftp = canonicalize_origin("ftp://example.com");
        assert!(ftp.is_err());
        let ok = canonicalize_origin("https://example.com");
        assert_eq!(ok.expect("origin"), "https://example.com");
    }
}
