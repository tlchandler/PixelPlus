//! Playlist sequencing: intro (once) → items (shuffled / repeated) → outro
//! (once), plus history for "previous".
//!
//! Pure logic (no I/O, no clocks) so it is fully unit-tested. The engine asks
//! the cursor for the current item, peeks at the next one (status, preloading,
//! DJ pre-rendering) and advances when an item ends. Visitor requests are
//! handled by the engine on top of the cursor (a request plays after the
//! current item, then the cursor continues where it was).

use pixelplus_core::model::{Playlist, PlaylistItem};
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

/// Which part of the playlist is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Intro,
    Main,
    Outro,
    Done,
}

/// A position in the playlist: phase + index into that phase's play order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub phase: Phase,
    /// Index into the phase's list (intro/outro) or the main play order.
    pub index: usize,
}

/// Walks a playlist.
#[derive(Debug, Clone)]
pub struct PlaylistCursor {
    playlist: Playlist,
    pos: Pos,
    /// Main-item play order (indices into `playlist.items`).
    order: Vec<usize>,
    history: Vec<(Pos, Vec<usize>)>,
    /// Finish after the current item: skip to the outro (schedule window end).
    ending: bool,
    rng: rand::rngs::StdRng,
}

impl PlaylistCursor {
    /// Start at the beginning (intro first, if any).
    pub fn new(playlist: Playlist) -> Self {
        Self::with_seed(playlist, rand::thread_rng().gen())
    }

    /// Deterministic shuffle (tests).
    pub fn with_seed(playlist: Playlist, seed: u64) -> Self {
        let mut c = PlaylistCursor {
            playlist,
            pos: Pos { phase: Phase::Intro, index: 0 },
            order: Vec::new(),
            history: Vec::new(),
            ending: false,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
        };
        c.order = c.make_order(None);
        c.normalize();
        c
    }

    /// Start directly at main item `index` (in playlist order), skipping the intro.
    pub fn start_at(&mut self, index: usize) {
        if index >= self.playlist.items.len() {
            return;
        }
        // Put the chosen item first; the rest keep (shuffled) order.
        self.order.retain(|&i| i != index);
        self.order.insert(0, index);
        self.pos = Pos { phase: Phase::Main, index: 0 };
        self.history.clear();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_done(&self) -> bool {
        self.pos.phase == Phase::Done
    }

    /// The item at the cursor (None when done).
    pub fn current(&self) -> Option<&PlaylistItem> {
        self.item_at(self.pos, &self.order)
    }

    /// Flattened 0-based index (intro, then main order, then outro) and total count.
    pub fn flat_index(&self) -> (u32, u32) {
        let p = &self.playlist;
        let count = (p.intro.len() + p.items.len() + p.outro.len()) as u32;
        let idx = match self.pos.phase {
            Phase::Intro => self.pos.index,
            Phase::Main => p.intro.len() + self.pos.index,
            Phase::Outro => p.intro.len() + p.items.len() + self.pos.index,
            Phase::Done => count as usize,
        };
        (idx as u32, count)
    }

    /// The item that follows the current one (None if the playlist ends).
    pub fn peek_next(&self) -> Option<&PlaylistItem> {
        let (pos, next_order) = self.successor();
        let order = next_order.as_ref().unwrap_or(&self.order);
        self.item_at(pos, order)
    }

    /// Advance to the next item. Returns the new current item.
    pub fn advance(&mut self) -> Option<&PlaylistItem> {
        if self.pos.phase == Phase::Done {
            return None;
        }
        self.history.push((self.pos, self.order.clone()));
        if self.history.len() > 200 {
            self.history.remove(0);
        }
        let (pos, new_cycle) = self.successor();
        if new_cycle.is_some() {
            // Same RNG state as the preview in `successor`, so the same order.
            let last = self.order.last().copied();
            self.order = make_order(&self.playlist, last, &mut self.rng);
        }
        self.pos = pos;
        self.normalize();
        self.current()
    }

    /// Step back to the previously played item. Returns the new current item.
    pub fn previous(&mut self) -> Option<&PlaylistItem> {
        if let Some((pos, order)) = self.history.pop() {
            self.pos = pos;
            self.order = order;
        }
        self.current()
    }

    /// Finish after the current item: continue with the outro (once), then end.
    /// Used when a schedule window ends with `finishSong`.
    pub fn finish_after_current(&mut self) {
        self.ending = true;
    }

    fn item_at<'a>(&'a self, pos: Pos, order: &[usize]) -> Option<&'a PlaylistItem> {
        let p = &self.playlist;
        match pos.phase {
            Phase::Intro => p.intro.get(pos.index),
            Phase::Main => order.get(pos.index).and_then(|&i| p.items.get(i)),
            Phase::Outro => p.outro.get(pos.index),
            Phase::Done => None,
        }
    }

    /// Where the cursor goes after the current item (and the new main order if a
    /// repeat cycle starts).
    fn successor(&self) -> (Pos, Option<Vec<usize>>) {
        let p = &self.playlist;
        let mut pos = self.pos;
        match pos.phase {
            Phase::Intro => pos.index += 1,
            Phase::Main => {
                if self.ending {
                    pos = Pos { phase: Phase::Outro, index: 0 };
                } else {
                    pos.index += 1;
                    if pos.index >= self.order.len() && p.repeat && !p.items.is_empty() {
                        // Deterministic preview of the next cycle: generated from a
                        // clone of the RNG; `advance` regenerates the same order.
                        let mut rng = self.rng.clone();
                        let order = make_order(p, self.order.last().copied(), &mut rng);
                        return (Pos { phase: Phase::Main, index: 0 }, Some(order));
                    }
                }
            }
            Phase::Outro => pos.index += 1,
            Phase::Done => {}
        }
        (normalized(p, pos, self.order.len()), None)
    }

    fn normalize(&mut self) {
        self.pos = normalized(&self.playlist, self.pos, self.order.len());
    }

    fn make_order(&mut self, last: Option<usize>) -> Vec<usize> {
        make_order(&self.playlist, last, &mut self.rng)
    }
}

/// Skip over empty phases.
fn normalized(p: &Playlist, mut pos: Pos, order_len: usize) -> Pos {
    loop {
        match pos.phase {
            Phase::Intro if pos.index >= p.intro.len() => pos = Pos { phase: Phase::Main, index: 0 },
            Phase::Main if pos.index >= order_len => pos = Pos { phase: Phase::Outro, index: 0 },
            Phase::Outro if pos.index >= p.outro.len() => pos = Pos { phase: Phase::Done, index: 0 },
            _ => return pos,
        }
    }
}

/// Play order for one cycle of the main items. With shuffle, the first item is
/// never the one that just played (no immediate repeats across cycles).
fn make_order(p: &Playlist, last: Option<usize>, rng: &mut impl Rng) -> Vec<usize> {
    let mut order: Vec<usize> = (0..p.items.len()).collect();
    if p.shuffle && order.len() > 1 {
        order.shuffle(rng);
        if let Some(last) = last {
            if order[0] == last {
                let j = rng.gen_range(1..order.len());
                order.swap(0, j);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(id: &str) -> PlaylistItem {
        PlaylistItem::Sequence { id: id.into(), sequence_id: id.into() }
    }

    fn pl(items: &[&str], intro: &[&str], outro: &[&str], shuffle: bool, repeat: bool) -> Playlist {
        Playlist {
            id: "p".into(),
            name: "P".into(),
            items: items.iter().map(|s| seq(s)).collect(),
            intro: intro.iter().map(|s| seq(s)).collect(),
            outro: outro.iter().map(|s| seq(s)).collect(),
            shuffle,
            repeat,
            crossfade_ms: 0,
        }
    }

    fn ids(c: &mut PlaylistCursor, n: usize) -> Vec<String> {
        let mut out = vec![];
        if let Some(i) = c.current() {
            out.push(i.id().to_string());
        }
        while out.len() < n {
            match c.advance() {
                Some(i) => out.push(i.id().to_string()),
                None => break,
            }
        }
        out
    }

    #[test]
    fn intro_items_outro_once() {
        let mut c = PlaylistCursor::with_seed(pl(&["a", "b"], &["i"], &["o"], false, false), 1);
        assert_eq!(ids(&mut c, 10), ["i", "a", "b", "o"]);
        assert!(c.is_done());
        assert!(c.advance().is_none());
    }

    #[test]
    fn repeat_skips_intro_and_never_reaches_outro() {
        let mut c = PlaylistCursor::with_seed(pl(&["a", "b"], &["i"], &["o"], false, true), 1);
        assert_eq!(ids(&mut c, 7), ["i", "a", "b", "a", "b", "a", "b"]);
    }

    #[test]
    fn finish_after_current_plays_outro() {
        let mut c = PlaylistCursor::with_seed(pl(&["a", "b", "c"], &[], &["o1", "o2"], false, true), 1);
        assert_eq!(c.current().unwrap().id(), "a");
        c.finish_after_current();
        assert_eq!(c.peek_next().unwrap().id(), "o1");
        assert_eq!(c.advance().unwrap().id(), "o1");
        assert_eq!(c.advance().unwrap().id(), "o2");
        assert!(c.advance().is_none());
    }

    #[test]
    fn shuffle_covers_all_and_no_immediate_repeat() {
        let items = ["a", "b", "c", "d", "e"];
        for seed in 0..50 {
            let mut c = PlaylistCursor::with_seed(pl(&items, &[], &[], true, true), seed);
            let played = ids(&mut c, 40);
            for cycle in played.chunks(5) {
                let mut s = cycle.to_vec();
                s.sort();
                assert_eq!(s, items, "each cycle plays every item once");
            }
            for w in played.windows(2) {
                assert_ne!(w[0], w[1], "no immediate repeat (seed {seed})");
            }
        }
    }

    #[test]
    fn peek_matches_advance_across_cycles() {
        for seed in 0..20 {
            let mut c = PlaylistCursor::with_seed(pl(&["a", "b", "c"], &[], &[], true, true), seed);
            for _ in 0..12 {
                let peek = c.peek_next().map(|i| i.id().to_string());
                let next = c.advance().map(|i| i.id().to_string());
                assert_eq!(peek, next);
            }
        }
    }

    #[test]
    fn previous_goes_back() {
        let mut c = PlaylistCursor::with_seed(pl(&["a", "b", "c"], &["i"], &[], false, false), 1);
        c.advance();
        c.advance();
        assert_eq!(c.current().unwrap().id(), "b");
        assert_eq!(c.previous().unwrap().id(), "a");
        assert_eq!(c.previous().unwrap().id(), "i");
        // Nothing earlier: stays.
        assert_eq!(c.previous().unwrap().id(), "i");
    }

    #[test]
    fn start_at_index_and_flat_index() {
        let mut c = PlaylistCursor::with_seed(pl(&["a", "b", "c"], &["i"], &["o"], false, false), 1);
        assert_eq!(c.flat_index(), (0, 5));
        c.start_at(2);
        assert_eq!(c.current().unwrap().id(), "c");
        assert_eq!(c.flat_index(), (1, 5));
        assert_eq!(ids(&mut c, 10), ["c", "a", "b", "o"]);
    }

    #[test]
    fn empty_playlists() {
        let mut c = PlaylistCursor::with_seed(pl(&[], &[], &[], true, true), 1);
        assert!(c.is_done());
        assert!(c.current().is_none());
        assert!(c.advance().is_none());
        let mut c = PlaylistCursor::with_seed(pl(&[], &["i"], &["o"], false, true), 1);
        assert_eq!(ids(&mut c, 5), ["i", "o"]);
    }
}
