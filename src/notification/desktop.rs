//! Desktop notifications on Linux via [`notify-rust`]. Errors are swallowed —
//! a missing notification daemon shouldn't stop transcription.

pub fn post(title: &str, body: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .appname("scribed")
        .timeout(notify_rust::Timeout::Milliseconds(2500))
        .show()
    {
        tracing::debug!(?e, "desktop notification failed");
    }
}
