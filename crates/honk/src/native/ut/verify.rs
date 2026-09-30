//! `HONK_MEMO_VERIFY`: recompute every memo hit and compare.
//!
//! With the variable set, each cache hit at a [`MemoSite`] is recomputed with
//! that one lookup bypassed and compared with the cached value. The cached
//! value is still returned, so a verified build must produce the same artifact
//! as a normal one. Hits met while a recompute is running are trusted, which
//! keeps the cost to about one extra evaluation per top-level hit. Counts are
//! process-wide; `memo_verify_report` summarizes them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoSite {
    Mint,
    CoreMint,
    Mull,
    Nest,
    Miss,
    Fuse,
    Crop,
    Fish,
    KtsgFold,
    BranSemi,
    Open,
    Burp,
    Redo,
    Rest,
}

const SITES: [MemoSite; 14] = [
    MemoSite::Mint,
    MemoSite::CoreMint,
    MemoSite::Mull,
    MemoSite::Nest,
    MemoSite::Miss,
    MemoSite::Fuse,
    MemoSite::Crop,
    MemoSite::Fish,
    MemoSite::KtsgFold,
    MemoSite::BranSemi,
    MemoSite::Open,
    MemoSite::Burp,
    MemoSite::Redo,
    MemoSite::Rest,
];

/// Mismatches printed per site before the rest are only counted.
const PRINTED_PER_SITE: u64 = 10;

static CHECKED: [AtomicU64; SITES.len()] = [const { AtomicU64::new(0) }; SITES.len()];
static MISMATCHED: [AtomicU64; SITES.len()] = [const { AtomicU64::new(0) }; SITES.len()];
static PERTURBED: AtomicU64 = AtomicU64::new(0);

fn index(site: MemoSite) -> usize {
    SITES
        .iter()
        .position(|s| *s == site)
        .expect("every site is listed")
}

/// The sites `HONK_MEMO_VERIFY` checks: every site for any value except a
/// comma-separated list of site names, which limits it to those.
fn sites() -> &'static Option<Vec<MemoSite>> {
    static SITES_ON: OnceLock<Option<Vec<MemoSite>>> = OnceLock::new();
    SITES_ON.get_or_init(|| {
        let raw = std::env::var("HONK_MEMO_VERIFY").ok()?;
        let named: Vec<MemoSite> = SITES
            .iter()
            .copied()
            .filter(|site| {
                raw.split(',')
                    .any(|name| name.trim() == format!("{site:?}"))
            })
            .collect();
        Some(if named.is_empty() {
            SITES.to_vec()
        } else {
            named
        })
    })
}

pub(crate) fn enabled() -> bool {
    sites().is_some()
}

pub(crate) fn enabled_for(site: MemoSite) -> bool {
    sites().as_ref().is_some_and(|on| on.contains(&site))
}

/// Per-`Ut` verification state.
#[derive(Default)]
pub(crate) struct MemoVerify {
    /// Nesting depth of recomputes; hits are verified only at depth 0.
    depth: u32,
    /// The site whose next lookup must miss, so a recompute reaches the
    /// uncached body.
    bypass: Option<MemoSite>,
}

impl MemoVerify {
    /// Whether a hit at `site` found now should be verified.
    pub(crate) fn due(&self, site: MemoSite) -> bool {
        self.depth == 0 && enabled_for(site)
    }

    /// Consumes a pending bypass for `site`; true means treat the lookup as a
    /// miss.
    pub(crate) fn take_bypass(&mut self, site: MemoSite) -> bool {
        if self.bypass == Some(site) {
            self.bypass = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn enter(&mut self, bypass: Option<MemoSite>) {
        self.depth += 1;
        self.bypass = bypass;
    }

    pub(crate) fn leave(&mut self) {
        self.depth -= 1;
        self.bypass = None;
    }
}

/// Records one verified hit. `detail` builds the message for a mismatch.
pub(crate) fn record(site: MemoSite, matched: bool, detail: impl FnOnce() -> String) {
    let i = index(site);
    CHECKED[i].fetch_add(1, Ordering::Relaxed);
    if !matched {
        let n = MISMATCHED[i].fetch_add(1, Ordering::Relaxed) + 1;
        if n <= PRINTED_PER_SITE {
            eprintln!("[memo-verify] {site:?} mismatch #{n}: {}", detail());
        }
    }
}

/// Records a recompute that changed the cache context key other than by bumping
/// the arm epoch, which would let the verified build diverge from a normal one.
pub(crate) fn record_perturbed(site: MemoSite, detail: impl FnOnce() -> String) {
    let n = PERTURBED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= PRINTED_PER_SITE {
        eprintln!(
            "[memo-verify] {site:?} recompute changed the cache context: {}",
            detail()
        );
    }
}

/// The per-site summary, or `None` when verification is off.
pub fn memo_verify_report() -> Option<String> {
    if !enabled() {
        return None;
    }
    let mut out = String::from("[memo-verify] site: checked hits, mismatches");
    let mut total_checked = 0;
    let mut total_mismatched = 0;
    for site in SITES {
        let i = index(site);
        let checked = CHECKED[i].load(Ordering::Relaxed);
        let mismatched = MISMATCHED[i].load(Ordering::Relaxed);
        total_checked += checked;
        total_mismatched += mismatched;
        if checked > 0 {
            out.push_str(&format!(
                "\n[memo-verify]   {site:?}: {checked}, {mismatched}"
            ));
        }
    }
    out.push_str(&format!(
        "\n[memo-verify] total: {total_checked} checked, {total_mismatched} mismatched, {} context changes",
        PERTURBED.load(Ordering::Relaxed)
    ));
    Some(out)
}

/// The first 160 characters of `text`, for mismatch messages.
pub(crate) fn brief(text: String) -> String {
    match text.char_indices().nth(160) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_is_consumed_once_and_only_for_its_site() {
        let mut verify = MemoVerify::default();
        verify.enter(Some(MemoSite::Mint));
        assert!(!verify.due(MemoSite::Mint));
        assert!(!verify.take_bypass(MemoSite::Burp));
        assert!(verify.take_bypass(MemoSite::Mint));
        assert!(!verify.take_bypass(MemoSite::Mint));
        verify.enter(None);
        verify.leave();
        verify.leave();
        assert_eq!(verify.depth, 0);
        assert_eq!(verify.bypass, None);
    }

    #[test]
    fn mismatches_are_counted_per_site() {
        let i = index(MemoSite::Rest);
        let checked = CHECKED[i].load(Ordering::Relaxed);
        let mismatched = MISMATCHED[i].load(Ordering::Relaxed);
        record(MemoSite::Rest, true, || unreachable!());
        record(MemoSite::Rest, false, || "differs".to_string());
        assert_eq!(CHECKED[i].load(Ordering::Relaxed), checked + 2);
        assert_eq!(MISMATCHED[i].load(Ordering::Relaxed), mismatched + 1);
        let perturbed = PERTURBED.load(Ordering::Relaxed);
        record_perturbed(MemoSite::Mint, String::new);
        assert_eq!(PERTURBED.load(Ordering::Relaxed), perturbed + 1);
    }

    #[test]
    fn brief_truncates_long_text() {
        assert_eq!(brief("short".to_string()), "short");
        let long = "x".repeat(200);
        let cut = brief(long);
        assert_eq!(cut.chars().count(), 161);
        assert!(cut.ends_with('…'));
    }
}
