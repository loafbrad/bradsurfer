//! Messages from Surfer to the page hosting it: the VS Code extension, or a web page
//! embedding Surfer in an iframe. See `surfer_notify_host` in `surfer/assets/integration.js`.

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    fn surfer_notify_host(message_json: &str);
}

/// Send `message` to the host page, if there is one. Does nothing outside the browser.
pub(crate) fn notify_host(message: &serde_json::Value) {
    #[cfg(target_arch = "wasm32")]
    surfer_notify_host(&message.to_string());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = message;
}
