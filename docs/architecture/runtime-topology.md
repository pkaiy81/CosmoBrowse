# Runtime topology

CosmoBrowse fixes the Browser/Renderer boundary with the following two-process model.

```text
+---------------------------+          Generic IPC (JSON message schema)
| Browser Process           | <-----------------------------------> +--------------------------+
| - adapter_native          |                                       | Renderer Process (WebView)|
| - session/history control |                                       | - frame tree drawing      |
| - network loader          |                                       | - diagnostics panel       |
| - crash dump writer       |                                       | - user input              |
+---------------------------+                                       +--------------------------+
            |                                                                    |
            | writes minimal crash dump                                           | reads IpcResponse::Page(BrowserPageDto)
            v                                                                    v
  /tmp/cosmobrowse-crash-report.json                                   network log / DOM snapshot / console
```

## Boundary rules
- Browser/Renderer communication uses framework-neutral schema (`IpcRequest`/`IpcResponse`).
- The Renderer depends only on DTOs (`BrowserPageDto`, `NavigationState`, `TabSummary` etc.) and never on `saba_app` internal structures.
- `dispatch_ipc` is the default command entrypoint, while per-command Tauri handlers are compatibility mode.
- On crash, the panic hook emits a minimal JSON dump and reproduction steps.
