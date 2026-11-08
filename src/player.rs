//! Previewing a track: one deck, play and pause, and seeking from the waveform.
//!
//! The audio device opens the first time something plays, not at launch.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use rbl_deck::{Deck, DeckEvent, Engine};
use rune::Waker;

#[derive(Default)]
pub struct Player {
    engine: Option<Engine>,
    errors: Option<Receiver<String>>,
    /// The track on the deck, by its library id. Rows move when the library reloads; ids don't.
    pub track: Option<String>,
    /// The track whose file the audio engine actually has. It can differ from `track` while a
    /// new track downloads: the deck shows the new one, the engine still holds the old one.
    loaded: Option<String>,
    request: u64,
    /// Play as soon as the load lands, from this point (0 to 1).
    pending: Option<f64>,
    pub playing: bool,
    /// Where the playhead is, 0 to 1.
    pub pos: f64,
    pub error: Option<String>,
    /// The audio output while a worker opens it, and a play asked for before it was ready.
    opening: Option<Receiver<Result<(Engine, Receiver<String>), rbl_deck::DeckError>>>,
    queued: Option<(String, PathBuf, Option<f64>)>,
    waker: Option<Waker>,
    /// The deck's track is coming down from iCloud and will play when it lands.
    pub fetching: bool,
    /// Where the playhead was and when, and whether it's moving. The waveforms draw from this
    /// and glide on their own between anchors, so the view needn't rebuild every frame.
    pub anchor: Anchor,
}

/// A known point on the playhead's path: at `at` it was at `pos` (0 to 1), moving at normal
/// speed or standing still. Each new anchor gets a new `seq`.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub seq: u64,
    pub pos: f64,
    pub at: Instant,
    pub moving: bool,
}

impl Default for Anchor {
    fn default() -> Self {
        Self { seq: 0, pos: 0.0, at: Instant::now(), moving: false }
    }
}

/// How far the engine may drift from the anchor's line before the waveforms jump to it.
const DRIFT_MS: f64 = 50.0;
/// A moving anchor is renewed this often, before its glide runs out.
const RENEW_SECS: f64 = 8.0;
/// How long one anchor's glide lasts. Longer than `RENEW_SECS`, so it never stops early.
pub const GLIDE_SECS: f64 = 16.0;

impl Anchor {
    /// Where the playhead should be now on this anchor's line, given the track's length.
    pub fn now(&self, total_ms: f64) -> f64 {
        if !self.moving || total_ms <= 0.0 {
            return self.pos;
        }
        (self.pos + self.at.elapsed().as_secs_f64() * 1000.0 / total_ms).min(1.0)
    }
}

impl Player {
    /// Opens the audio output on a worker. Opening a device can take a while (a Bluetooth
    /// speaker waking up), and the window must never wait for it. Called once at launch.
    pub fn open(&mut self, waker: Option<Waker>) {
        if self.engine.is_some() || self.opening.is_some() || rune::tooling::headless_run().is_some() {
            return;
        }
        let (tx, rx) = channel();
        let w = waker.clone();
        std::thread::spawn(move || {
            let (etx, erx) = channel();
            let ew = w.clone();
            let sink: rbl_deck::EventSink = Arc::new(move |e| {
                if let DeckEvent::Error { message, .. } = e {
                    let _ = etx.send(message);
                    if let Some(w) = &ew {
                        w();
                    }
                }
            });
            let opened = Engine::new(&sink).map(|e| {
                // The timing runs play silently.
                if std::env::var_os("SLEEVE_AUTOPLAY").is_some() {
                    e.master().set_gain(0.0);
                }
                (e, erx)
            });
            if tx.send(opened).is_ok()
                && let Some(w) = w
            {
                w();
            }
        });
        self.opening = Some(rx);
        self.waker = waker;
    }

    /// The engine, once the worker has opened it.
    fn engine(&mut self) -> Option<&Engine> {
        if self.engine.is_none()
            && let Some(rx) = &self.opening
            && let Ok(opened) = rx.try_recv()
        {
            self.opening = None;
            match opened {
                Ok((e, errors)) => {
                    self.engine = Some(e);
                    self.errors = Some(errors);
                }
                Err(e) => self.error = Some(format!("Couldn't open the audio output: {e}")),
            }
        }
        self.engine.as_ref()
    }

    /// Shows a track as playing with no audio behind it, for headless scenes.
    pub fn stage(&mut self, id: &str, pos: f64) {
        self.track = Some(id.to_owned());
        self.playing = true;
        self.pos = pos;
        // Moving, so headless shots at different times show the waveforms glide.
        self.reanchor(pos, true);
    }

    /// Starts a new anchor here and now.
    fn reanchor(&mut self, pos: f64, moving: bool) {
        self.anchor = Anchor { seq: self.anchor.seq + 1, pos, at: Instant::now(), moving };
    }

    /// The playhead now, as best known: on the anchor's line while it moves.
    pub fn now(&self) -> f64 {
        match self.duration_ms() {
            Some(total) if self.anchor.moving => self.anchor.now(total),
            _ => self.pos,
        }
    }

    pub fn is_on(&self, id: &str) -> bool {
        self.track.as_deref() == Some(id)
    }

    /// The loaded track's length in milliseconds, once the deck knows it.
    pub fn duration_ms(&self) -> Option<f64> {
        if !self.engine_has_deck_track() {
            return None;
        }
        let snap = self.engine.as_ref()?.snapshot();
        if snap.a.load_id != self.request {
            return None;
        }
        (snap.a.total_frames > 0 && snap.sample_rate > 0).then(|| snap.a.total_frames as f64 * 1000.0 / f64::from(snap.sample_rate))
    }

    /// Plays `row` from `at` (0 to 1), loading it first if it isn't on the deck.
    pub fn play(&mut self, id: &str, path: PathBuf, at: Option<f64>) {
        self.error = None;
        if self.engine().is_none() {
            // Still opening: play as soon as it's open.
            if self.opening.is_some() {
                self.track = Some(id.to_owned());
                self.pos = at.unwrap_or(0.0);
                self.playing = true;
                self.queued = Some((id.to_owned(), path, at));
            }
            return;
        }
        if self.fetching && !self.is_on(id) {
            self.fetching = false;
        }
        // Only the engine's own track counts as "the same": the deck may already show this one
        // while the engine still holds the last.
        let same = self.loaded.as_deref() == Some(id);
        if !same {
            self.request += 1;
            let request = self.request;
            let Some(engine) = self.engine() else { return };
            engine.pause(Deck::A);
            engine.load_as(Deck::A, &path, request);
            self.loaded = Some(id.to_owned());
            self.track = Some(id.to_owned());
            self.pos = at.unwrap_or(0.0);
            self.pending = Some(self.pos);
            self.playing = true;
            self.reanchor(self.pos, false);
            return;
        }
        let Some(engine) = self.engine() else { return };
        let snap = engine.snapshot().a;
        if !snap.loaded {
            self.pending = Some(at.unwrap_or(self.pos));
            self.playing = true;
            return;
        }
        if let Some(at) = at {
            seek(engine, snap, at);
        }
        engine.play(Deck::A);
        if let Some(at) = at {
            self.pos = at;
        }
        self.playing = true;
        let from = at.unwrap_or(self.pos);
        self.reanchor(from, true);
    }

    pub fn pause(&mut self) {
        let here = self.now();
        self.pos = here;
        self.reanchor(here, false);
        self.pending = None;
        if let Some(e) = &self.engine {
            e.pause(Deck::A);
        }
        self.playing = false;
    }

    /// Moves the playhead without changing whether it plays.
    pub fn seek(&mut self, at: f64) {
        self.pos = at;
        let moving = self.anchor.moving;
        self.reanchor(at, moving);
        if self.pending.is_some() {
            self.pending = Some(at);
            return;
        }
        if let Some(e) = &self.engine {
            seek(e, e.snapshot().a, at);
        }
    }

    /// Puts a track on the deck while its file downloads, so the deck shows it straight away.
    pub fn fetch(&mut self, id: &str) {
        self.pause();
        self.track = Some(id.to_owned());
        self.pos = 0.0;
        self.reanchor(0.0, false);
        self.fetching = true;
        self.error = None;
    }

    pub fn stop(&mut self) {
        self.fetching = false;
        self.pause();
        self.track = None;
        self.pos = 0.0;
        self.reanchor(0.0, false);
    }

    /// Whether the engine holds the track the deck shows. While a new track downloads, it
    /// doesn't, and nothing about the old one (length, position) may leak onto the deck.
    fn engine_has_deck_track(&self) -> bool {
        self.track.is_some() && self.loaded == self.track
    }

    /// Reads the deck. Returns true when the playing state or the anchor changed; the readouts
    /// that follow the position are the app's to compare.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.errors
            && let Ok(msg) = rx.try_recv()
        {
            self.error = Some(format!("Couldn't play this track: {msg}"));
            self.playing = false;
            self.pending = None;
            changed = true;
        }
        if self.engine.is_none() && self.opening.is_some() {
            let _ = self.engine();
            if self.engine.is_some() {
                changed = true;
                if let Some((id, path, at)) = self.queued.take() {
                    self.play(&id, path, at);
                }
            }
        }
        let Some(engine) = &self.engine else { return changed };
        let snap = engine.snapshot().a;
        if let Some(at) = self.pending
            && snap.loaded
            && snap.load_id == self.request
        {
            self.pending = None;
            seek(engine, snap, at);
            engine.play(Deck::A);
        }
        if self.pending.is_none() && self.engine_has_deck_track() && snap.load_id == self.request {
            let playing = snap.playing && !snap.played_out;
            changed |= playing != self.playing;
            self.playing = playing;
            if snap.total_frames > 0 {
                let pos = (snap.position_frames as f64 / snap.total_frames as f64).clamp(0.0, 1.0);
                self.pos = pos;
                let total = self.duration_ms().unwrap_or(0.0);
                let line = self.anchor.now(total);
                if playing != self.anchor.moving || (pos - line).abs() * total > DRIFT_MS {
                    // Started, stopped, or moved off the line (a seek, a stall): jump to the engine.
                    self.reanchor(pos, playing);
                    changed = true;
                } else if playing && self.anchor.at.elapsed().as_secs_f64() > RENEW_SECS {
                    // Still on the line: renew from the line itself, so nothing visibly jumps.
                    self.reanchor(line, true);
                    changed = true;
                }
            }
        }
        changed
    }
}

fn seek(engine: &Engine, _snap: rbl_deck::DeckSnapshot, at: f64) {
    let snap = engine.snapshot();
    if snap.a.total_frames > 0 && snap.sample_rate > 0 {
        let ms = at.clamp(0.0, 1.0) * snap.a.total_frames as f64 * 1000.0 / f64::from(snap.sample_rate);
        engine.seek_ms(Deck::A, ms);
    }
}

#[cfg(test)]
mod probe {
    /// Plays a real track for two seconds and prints what reached the output. Read-only.
    /// `SLEEVE_PROBE=/path/to/song cargo test probe_sound -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_sound() {
        let path = std::env::var("SLEEVE_PROBE").unwrap();
        let sink: rbl_deck::EventSink = std::sync::Arc::new(|e| println!("event {e:?}"));
        let e = rbl_deck::Engine::new(&sink).unwrap();
        e.load_as(rbl_deck::Deck::A, std::path::Path::new(&path), 1);
        for _ in 0..50 {
            if e.snapshot().a.loaded { break }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        e.seek_ms(rbl_deck::Deck::A, 60_000.0);
        e.play(rbl_deck::Deck::A);
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let s = e.snapshot();
            println!("pos {} playing {} rate {} gain {} peaks {:?} rms {:?} health {:?}", s.a.position_frames, s.a.playing, s.sample_rate, e.master().gain(), e.master().peaks(), e.master().rms(), e.audio_health());
        }
    }
}
