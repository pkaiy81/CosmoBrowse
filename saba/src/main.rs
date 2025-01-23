#![no_std]
#![no_main]

extern crate alloc;

use crate::alloc::string::ToString;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;
use net_wasabi::http::HttpClient;
use noli::*;
use saba_core::browser::Browser;
use saba_core::error::Error;
use saba_core::http::HttpResponse;
use saba_core::url::Url;
use ui_wasabi::app::WasabiUI;

fn handle_url(url: String) -> Result<HttpResponse, Error> {
    // Interpret the URL.
    let parsed_url = match Url::new(url.to_string()).parse() {
        // 1
        Ok(url) => url,
        Err(e) => {
            return Err(Error::UnexpectedInput(format!(
                "input html is not supported: {:?}",
                e
            )));
        }
    };

    // Send HTTP request.
    let client = HttpClient::new();
    let response = match client.get(
        // 2
        parsed_url.host(),
        parsed_url.port().parse::<u16>().expect(&format!(
            // 3
            "port number should be u16 but got {}",
            parsed_url.port()
        )),
        parsed_url.path(),
    ) {
        Ok(res) => {
            // 4
            // When the status code of HTTP response is 302, Transfer to the location specified in the Location header(Redirect).
            if res.status_code() == 302 {
                // 5
                let location = match res.header_value("Location") {
                    // 6
                    Ok(value) => value,
                    Err(_) => return Ok(res),
                };
                let redirect_parsed_url = Url::new(location); // 7

                let redirect_res = match client.get(
                    // 8
                    redirect_parsed_url.host(),
                    redirect_parsed_url.port().parse::<u16>().expect(&format!(
                        "port number should be u16 but got {}",
                        parsed_url.port()
                    )),
                    redirect_parsed_url.path(),
                ) {
                    Ok(res) => res,
                    Err(e) => return Err(Error::Network(format!("{:?}", e))),
                };

                redirect_res
            } else {
                // 9
                res
            }
        }
        Err(e) => {
            return Err(Error::Network(format!(
                "failed to get http response: {:?}",
                e
            )))
        }
    };
    Ok(response)
}

fn main() -> u64 {
    // Initialize the browser.
    let browser = Browser::new();

    // Initialize WasabiUI struct.
    let ui = Rc::new(RefCell::new(WasabiUI::new(browser)));

    // Start the application.
    match ui.borrow_mut().start(handle_url) {
        Ok(_) => {}
        Err(e) => {
            println!("browser fails to start: {:?}", e);
            return 1;
        }
    };

    0
}

entry_point!(main);
