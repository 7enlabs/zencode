//! Property-based tests for pure functions in `zen-core`.
//!
//! Hand-rolled property tests (deterministic LCG, fixed seeds) so the suite
//! needs no new dev-dependencies. Each test states an invariant that must
//! hold for every generated input, not just hand-picked examples.
//!
//! Functions under test: `cache_hit_rate`, `split_stable_volatile`.

use zen_core::{cache_hit_rate, split_stable_volatile, UsageStats, CACHE_VOLATILE_MARKER};

/// Deterministic pseudo-random generator (64-bit LCG, Knuth constants).
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    /// Value in `0..bound` (bound must be > 0).
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

const CASES: u64 = 2_000;

// ---------- cache_hit_rate ----------

#[test]
fn prop_cache_hit_rate_always_bounded_and_finite() {
    let mut rng = Lcg::new(0x2E1C_0FF1);
    for _ in 0..CASES {
        let cache_read = rng.next_u64();
        let prompt = rng.next_u64();
        let rate = cache_hit_rate(cache_read, prompt);
        assert!(rate.is_finite(), "rate not finite: {rate}");
        assert!((0.0..=1.0).contains(&rate), "rate out of [0,1]: {rate}");
    }
}

#[test]
fn prop_cache_hit_rate_zero_denominator_is_zero() {
    let mut rng = Lcg::new(0x0B17_CA11);
    for _ in 0..CASES {
        let cache_read = rng.next_u64();
        assert_eq!(cache_hit_rate(cache_read, 0), 0.0);
    }
    assert_eq!(cache_hit_rate(0, 0), 0.0);
}

#[test]
fn prop_cache_hit_rate_exact_ratio_below_cap() {
    let mut rng = Lcg::new(0xCA11_BA11);
    for _ in 0..CASES {
        let prompt = rng.below(1_000_000) + 1; // 1..=1_000_000
        let cache_read = rng.below(prompt + 1); // 0..=prompt
        let expected = cache_read as f64 / prompt as f64;
        let rate = cache_hit_rate(cache_read, prompt);
        assert!(
            (rate - expected).abs() < 1e-12,
            "cache_read={cache_read} prompt={prompt}: {rate} != {expected}"
        );
    }
}

#[test]
fn prop_cache_hit_rate_caps_at_one() {
    let mut rng = Lcg::new(0xCAF5_1E55);
    for _ in 0..CASES {
        let prompt = rng.below(1_000_000) + 1;
        let cache_read = prompt + rng.below(1_000_000);
        assert_eq!(cache_hit_rate(cache_read, prompt), 1.0);
    }
}

#[test]
fn prop_cache_hit_rate_monotonic_in_cache_read() {
    let mut rng = Lcg::new(0xA070_0001);
    for _ in 0..CASES {
        let prompt = rng.below(1_000_000) + 1;
        let a = rng.below(2_000_000);
        let b = a + rng.below(2_000_000);
        assert!(cache_hit_rate(a, prompt) <= cache_hit_rate(b, prompt));
    }
}

#[test]
fn prop_usage_stats_method_matches_free_function() {
    let mut rng = Lcg::new(0x05A6E);
    for _ in 0..CASES {
        let stats = UsageStats {
            input_tokens: rng.next_u64(),
            output_tokens: rng.next_u64(),
            cache_read_input_tokens: rng.next_u64(),
        };
        assert_eq!(
            stats.cache_hit_rate(),
            cache_hit_rate(stats.cache_read_input_tokens, stats.input_tokens)
        );
    }
}

// ---------- split_stable_volatile ----------

/// Random short string over an alphabet that includes marker fragments, so
/// generated inputs sometimes contain the real marker by construction below.
fn random_text(rng: &mut Lcg) -> String {
    const PIECES: &[&str] = &[
        "a",
        "bb",
        "prompt",
        "\n",
        " ",
        "zen-cache",
        "<!--",
        "-->",
        "x",
    ];
    let n = rng.below(24);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(PIECES[rng.below(PIECES.len() as u64) as usize]);
    }
    s
}

#[test]
fn prop_split_without_marker_is_identity() {
    let mut rng = Lcg::new(0x5_9117);
    for _ in 0..CASES {
        let text = random_text(&mut rng);
        if text.contains(CACHE_VOLATILE_MARKER) {
            continue;
        }
        let (stable, volatile) = split_stable_volatile(&text);
        assert_eq!(stable, text);
        assert_eq!(volatile, "");
    }
}

#[test]
fn prop_split_with_marker_roundtrips() {
    let mut rng = Lcg::new(0x0002_00DD);
    for _ in 0..CASES {
        let before = random_text(&mut rng);
        let after = random_text(&mut rng);
        if before.contains(CACHE_VOLATILE_MARKER) {
            continue;
        }
        let original = format!("{before}{CACHE_VOLATILE_MARKER}{after}");
        let (stable, volatile) = split_stable_volatile(&original);
        assert_eq!(stable, before);
        assert_eq!(volatile, after);
        assert_eq!(
            format!("{stable}{CACHE_VOLATILE_MARKER}{volatile}"),
            original,
            "split must reconstruct the original prompt"
        );
    }
}

#[test]
fn prop_split_stable_never_contains_marker() {
    let mut rng = Lcg::new(0x0057_AB1E);
    for _ in 0..CASES {
        let mut text = random_text(&mut rng);
        if rng.below(2) == 0 {
            text.push_str(CACHE_VOLATILE_MARKER);
            text.push_str(&random_text(&mut rng));
        }
        let (stable, _) = split_stable_volatile(&text);
        assert!(!stable.contains(CACHE_VOLATILE_MARKER));
    }
}

#[test]
fn prop_split_uses_first_marker_only() {
    let mut rng = Lcg::new(0x000F_1257);
    for _ in 0..CASES {
        let head = random_text(&mut rng);
        let mid = random_text(&mut rng);
        let tail = random_text(&mut rng);
        if head.contains(CACHE_VOLATILE_MARKER) {
            continue;
        }
        let original = format!("{head}{CACHE_VOLATILE_MARKER}{mid}{CACHE_VOLATILE_MARKER}{tail}");
        let (stable, volatile) = split_stable_volatile(&original);
        assert_eq!(stable, head);
        assert!(
            volatile.contains(CACHE_VOLATILE_MARKER),
            "second marker must stay in the volatile part"
        );
        assert_eq!(
            format!("{stable}{CACHE_VOLATILE_MARKER}{volatile}"),
            original
        );
    }
}
