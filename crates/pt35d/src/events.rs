//! The sway event stream.
//!
//! Polling the tree every two seconds is what made a closed window keep its
//! slot in the dock. sway will tell us instead: one connection of its own, kept
//! apart from the one commands go out on, because replies and events share a
//! socket and would interleave.

use anyhow::Result;
use pt35_common::ipc::Event;
use pt35_common::sway::Sway;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::server::Subscribers;
use crate::state::Session;

/// What we ask sway for. `workspace` covers the focus moving between apps,
/// `window` covers everything else.
const EVENTS: &[&str] = &["window", "workspace"];

/// Events arriving inside this window are served by one tree read. An app that
/// owns a dialog produces several at once when it goes.
const COALESCE: Duration = Duration::from_millis(50);

/// Changes worth a redraw.
///
/// Not `title`: the dock draws the app id, so a build scrolling past in a
/// terminal would cost three sway round trips a frame and change nothing. The
/// window picker does show titles and reads them on its own timer.
const INTERESTING: &[&str] = &[
    "new",
    "close",
    "focus",
    "move",
    "floating",
    "fullscreen_mode",
    "init",
    "empty",
    "reload",
];

pub fn run(session: Arc<Mutex<Session>>, subscribers: Arc<Subscribers>) {
    loop {
        if let Err(e) = stream(&session, &subscribers) {
            log::debug!("sway event stream ended: {e}");
        }
        // sway going away is the session ending; the poll thread notices and
        // takes the process down with it. Until then, try again.
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn stream(session: &Arc<Mutex<Session>>, subscribers: &Arc<Subscribers>) -> Result<()> {
    let mut sway = Sway::connect_untimed()?;
    sway.subscribe(EVENTS)?;
    log::info!("subscribed to sway {EVENTS:?} events");
    // A thread of its own does the reading, so this one can wait out a burst and
    // take everything in it at once.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((_, body)) = sway.next_event() {
            if tx.send(body).is_err() {
                break;
            }
        }
    });
    loop {
        let first = rx.recv()?;
        let mut news = News::default();
        news.add(&first);
        if news.is_none() {
            continue;
        }
        std::thread::sleep(COALESCE);
        for body in rx.try_iter() {
            news.add(&body);
        }
        let mut session = session.lock().expect("session");
        if news.reload {
            session.after_sway_reload();
        }
        session.refresh_windows();
        // Under the lock, so a status read after this one cannot go out before it.
        subscribers.broadcast(&Event::Status(session.status.clone()));
        for note in &session.take_notifications() {
            subscribers.broadcast(note);
        }
    }
}

/// What a burst of events asks for.
#[derive(Default)]
struct News {
    refresh: bool,
    reload: bool,
}

impl News {
    fn add(&mut self, body: &str) {
        self.refresh |= worth_refreshing(body);
        self.reload |= is_reload(body);
    }

    fn is_none(&self) -> bool {
        !self.refresh && !self.reload
    }
}

fn is_reload(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .is_ok_and(|value| value["change"].as_str() == Some("reload"))
}

/// Every sway event carries a `change` saying what happened.
fn worth_refreshing(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        // An event we cannot read is still an event. Better a redraw than a
        // missed close.
        return true;
    };
    match value["change"].as_str() {
        Some(change) => INTERESTING.contains(&change),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_is_worth_a_redraw_and_a_title_is_not() {
        assert!(worth_refreshing(
            r#"{"change":"close","container":{"id":4}}"#
        ));
        assert!(worth_refreshing(r#"{"change":"new"}"#));
        assert!(worth_refreshing(r#"{"change":"focus"}"#));
        // The dock draws the app id. A prompt redraw is not news.
        assert!(!worth_refreshing(r#"{"change":"title"}"#));
        assert!(!worth_refreshing(r#"{"change":"mark"}"#));
        assert!(!worth_refreshing(r#"{"change":"urgent"}"#));
    }

    #[test]
    fn a_config_reload_is_told_apart() {
        let reload = r#"{"change":"reload","old":null,"current":null}"#;
        assert!(is_reload(reload));
        assert!(!is_reload(r#"{"change":"focus"}"#));
        let mut news = News::default();
        news.add(r#"{"change":"title"}"#);
        assert!(news.is_none());
        news.add(reload);
        assert!(news.reload);
    }

    #[test]
    fn an_unreadable_event_still_refreshes() {
        assert!(worth_refreshing("not json"));
        assert!(worth_refreshing("{}"));
    }
}
