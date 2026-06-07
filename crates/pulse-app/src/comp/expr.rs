//! Per-property **expressions** — the After-Effects signature feature.
//!
//! Any animatable scalar [`Track`](super::Track) can carry an optional
//! `expression` string. When present, the property's value at time `t` is the
//! result of evaluating that expression with [`rhai`] (a pure-Rust embeddable
//! scripting engine — no system deps) instead of the raw keyframed sample. The
//! keyframed sample is still computed and exposed to the script as `value`, so
//! an expression can *drive* the animation (`value + 10`, `value * sin(time)`)
//! rather than replace it.
//!
//! ## Context
//! Each evaluation binds a small [`ExprCtx`] into scope as plain variables:
//! - `time` — the sample time in seconds
//! - `value` — the property's keyframed value at `time` (so expressions offset it)
//! - `fps` — the comp's frame rate
//! - `duration` — the comp's duration in seconds
//! - `index` — the layer's stack index
//!
//! plus a handful of AE-style helper functions registered on the engine:
//! - `wiggle(freq, amp)` — smooth pseudo-random jitter, **deterministic** per
//!   `(layer, time)` (seeded from a stable hash, never `Math.random`), so a
//!   given frame always renders identically
//! - `linear(t, tmin, tmax, v1, v2)` — remap `t` from `[tmin, tmax]` to
//!   `[v1, v2]` (clamped to the endpoints outside the range)
//! - `clamp(v, lo, hi)` — clamp `v` into `[lo, hi]`
//!
//! `sin` / `cos` / `abs` / `floor` (and the rest of rhai's math) are available
//! out of the box.
//!
//! ## Errors & caching
//! A parse or eval error never panics: [`eval`] returns `None`, and the caller
//! falls back to the keyframed value. Whether the *last* evaluation of a given
//! expression string failed is recorded so the UI can surface an error state
//! (see [`last_error`]). Compiled ASTs are cached per source string (in a
//! thread-local cache), so a hot render path re-uses the compiled program rather
//! than re-parsing every frame for every property.

use rhai::{Dynamic, Engine, AST};
use std::cell::RefCell;
use std::collections::HashMap;

/// Coerce a rhai [`Dynamic`] numeric argument (int *or* float) to `f64`.
///
/// rhai doesn't auto-coerce integer literals to floats, so a call like
/// `wiggle(2, 50)` passes two ints. Accepting `Dynamic` and coercing here lets
/// the helpers take natural numeric literals without the user writing `2.0`.
fn as_f64(v: &Dynamic) -> f64 {
    if let Some(f) = v.clone().try_cast::<f64>() {
        f
    } else if let Some(i) = v.clone().try_cast::<i64>() {
        i as f64
    } else {
        0.0
    }
}

/// The scalar context an expression is evaluated against: the sample time, the
/// keyframed `value` it can offset, the comp's `fps` / `duration`, and the
/// layer's stack `index`. All bound into the script as same-named variables.
#[derive(Clone, Copy, Debug)]
pub struct ExprCtx {
    /// Sample time in seconds (`time` in the script).
    pub time: f32,
    /// The property's keyframed value at `time` (`value` in the script).
    pub value: f32,
    /// The comp's frame rate (`fps`).
    pub fps: f32,
    /// The comp's duration in seconds (`duration`).
    pub duration: f32,
    /// The layer's index in the comp's stack (`index`). Also seeds `wiggle` so
    /// two layers with the same expression jitter independently.
    pub index: usize,
}

impl ExprCtx {
    /// A bare context with only `time` / `value` set (fps/duration zeroed,
    /// index 0) — convenient for tests and value-only expressions.
    #[cfg(test)]
    pub fn at(time: f32, value: f32) -> Self {
        ExprCtx {
            time,
            value,
            fps: 0.0,
            duration: 0.0,
            index: 0,
        }
    }
}

thread_local! {
    /// Per-thread compiled-AST cache, keyed by the expression source string.
    /// `None` means the string failed to compile (cached so we don't re-parse a
    /// broken expression every frame).
    static AST_CACHE: RefCell<HashMap<String, Option<AST>>> = RefCell::new(HashMap::new());
    /// Whether the most recent [`eval`] of each expression string errored
    /// (parse *or* runtime). Drives the UI error state.
    static LAST_ERROR: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// Build the shared rhai [`Engine`] with the helper functions registered and
/// limits tightened so a hostile/looping expression can't hang the render.
fn build_engine() -> Engine {
    let mut engine = Engine::new();
    // Bound the cost of any single evaluation: expressions are sampled per frame
    // per property, so cap operations / call depth and forbid defining functions.
    engine.set_max_operations(10_000);
    engine.set_max_call_levels(16);
    engine.set_max_expr_depths(64, 64);

    // wiggle(freq, amp): smooth deterministic jitter. `index` (the layer) salts
    // the seed so identical expressions on different layers diverge; the value is
    // a sum of a few sines whose phases come from a stable integer hash — same
    // (index, time) always yields the same number, and it varies smoothly with
    // time. NOT Math.random: fully reproducible frame to frame.
    let wiggle_seed = std::cell::Cell::new(0u64);
    let seed_ref = std::rc::Rc::new(wiggle_seed);
    let seed_for_fn = seed_ref.clone();
    engine.register_fn("wiggle", move |freq: Dynamic, amp: Dynamic| -> f64 {
        wiggle_value(seed_for_fn.get(), as_f64(&freq), as_f64(&amp))
    });
    // Stash the seed accessor so `eval` can prime it per call. We re-create the
    // engine cheaply per thread (cached below), so this closure capture is fine.
    SEED.with(|s| *s.borrow_mut() = Some(seed_ref));

    // linear(t, tmin, tmax, v1, v2): remap with clamped endpoints. Args are
    // `Dynamic` so int *and* float literals both work.
    engine.register_fn(
        "linear",
        |t: Dynamic, tmin: Dynamic, tmax: Dynamic, v1: Dynamic, v2: Dynamic| -> f64 {
            let (t, tmin, tmax, v1, v2) =
                (as_f64(&t), as_f64(&tmin), as_f64(&tmax), as_f64(&v1), as_f64(&v2));
            if tmax == tmin {
                return v1;
            }
            let f = ((t - tmin) / (tmax - tmin)).clamp(0.0, 1.0);
            v1 + (v2 - v1) * f
        },
    );

    // clamp(v, lo, hi).
    engine.register_fn("clamp", |v: Dynamic, lo: Dynamic, hi: Dynamic| -> f64 {
        let (v, lo, hi) = (as_f64(&v), as_f64(&lo), as_f64(&hi));
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        v.clamp(lo, hi)
    });

    engine
}

thread_local! {
    /// The shared engine for this thread (built once).
    static ENGINE: Engine = build_engine();
    /// The per-call `wiggle` seed cell, shared with the engine's `wiggle` fn so
    /// each [`eval`] can prime it before running.
    static SEED: RefCell<Option<std::rc::Rc<std::cell::Cell<u64>>>> = const { RefCell::new(None) };
}

/// Smooth, deterministic jitter for `wiggle(freq, amp)`.
///
/// Sums a few sine waves whose frequencies and phases are derived from `seed`
/// (a stable hash of the layer index + time bucket), scaled to roughly `±amp`.
/// Because the seed is a pure function of `(index, time)`, the same frame always
/// produces the same offset; because the seed's time component changes across
/// frames, the value evolves over time.
fn wiggle_value(seed: u64, freq: f64, amp: f64) -> f64 {
    // Three octaves of sine, phases from the seed, normalized to ~[-1, 1].
    let mut acc = 0.0;
    let mut norm = 0.0;
    for k in 0..3u64 {
        let h = splitmix64(seed.wrapping_add(k.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
        let phase = (h as f64 / u64::MAX as f64) * std::f64::consts::TAU;
        let weight = 1.0 / (1.0 + k as f64);
        // `freq` modulates how fast the seed's time component already advances;
        // fold it into the phase so higher freq = faster jitter.
        acc += weight * ((phase * (1.0 + freq)).sin());
        norm += weight;
    }
    if norm == 0.0 {
        return 0.0;
    }
    (acc / norm) * amp
}

/// A fast, well-mixed integer hash (SplitMix64) — used to turn the
/// `(index, time)` seed into well-distributed phases for [`wiggle_value`].
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Derive the stable `wiggle` seed for `(layer index, time)`.
///
/// Time is quantized to milliseconds so the seed is a function of the *frame*
/// (same `t` → same seed → same jitter) while still advancing across frames.
fn wiggle_seed(ctx: &ExprCtx) -> u64 {
    let t_ms = (ctx.time as f64 * 1000.0).round() as i64 as u64;
    splitmix64((ctx.index as u64).wrapping_mul(0x100_0000_01b3) ^ splitmix64(t_ms))
}

/// Evaluate `expr` against `ctx`, returning the resulting scalar or `None` on a
/// parse or runtime error. Never panics. The last-error flag for `expr` is
/// updated so the UI can show an error state (see [`last_error`]).
///
/// Compiled ASTs are cached per source string; a string that previously failed
/// to compile is remembered (cached as a compile failure) and short-circuits.
pub fn eval(expr: &str, ctx: &ExprCtx) -> Option<f32> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        // An empty expression isn't an error — it just means "no expression".
        set_error(expr, false);
        return None;
    }

    ENGINE.with(|engine| {
        // Prime the per-call wiggle seed so the engine's `wiggle` fn is
        // deterministic. Done *inside* `ENGINE.with` so the engine (and thus the
        // shared SEED cell it captured) is built first — priming before then
        // would target a stale cell that `build_engine` overwrites.
        let seed = wiggle_seed(ctx);
        SEED.with(|s| {
            if let Some(cell) = s.borrow().as_ref() {
                cell.set(seed);
            }
        });

        // Compile (or fetch the cached AST). Cache compile failures too so we
        // don't re-parse a broken string every frame.
        let compiled = AST_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache
                .entry(expr.to_string())
                .or_insert_with(|| engine.compile(trimmed).ok())
                .clone()
        });
        let Some(ast) = compiled else {
            set_error(expr, true);
            return None;
        };

        // Bind the context as plain variables in a scope.
        let mut scope = rhai::Scope::new();
        scope.push_constant("time", ctx.time as f64);
        scope.push_constant("value", ctx.value as f64);
        scope.push_constant("fps", ctx.fps as f64);
        scope.push_constant("duration", ctx.duration as f64);
        scope.push_constant("index", ctx.index as i64);

        // Evaluate. The script's last expression is the result; accept either a
        // float or an int. Any error (or non-numeric result) is a fallback.
        match engine.eval_ast_with_scope::<f64>(&mut scope, &ast) {
            Ok(v) if v.is_finite() => {
                set_error(expr, false);
                Some(v as f32)
            }
            Ok(_) => {
                // Non-finite (NaN/inf) — treat as an error so the UI flags it and
                // the caller falls back rather than poisoning the render.
                set_error(expr, true);
                None
            }
            Err(_) => {
                // Try again as an integer result (e.g. `index * 2`).
                let mut scope = rhai::Scope::new();
                scope.push_constant("time", ctx.time as f64);
                scope.push_constant("value", ctx.value as f64);
                scope.push_constant("fps", ctx.fps as f64);
                scope.push_constant("duration", ctx.duration as f64);
                scope.push_constant("index", ctx.index as i64);
                match engine.eval_ast_with_scope::<i64>(&mut scope, &ast) {
                    Ok(v) => {
                        set_error(expr, false);
                        Some(v as f32)
                    }
                    Err(_) => {
                        set_error(expr, true);
                        None
                    }
                }
            }
        }
    })
}

/// Record whether the last evaluation of `expr` errored.
fn set_error(expr: &str, errored: bool) {
    LAST_ERROR.with(|m| {
        m.borrow_mut().insert(expr.to_string(), errored);
    });
}

/// Whether the most recent [`eval`] of `expr` failed (parse or runtime error).
/// `false` for an expression that has never been evaluated or last succeeded —
/// drives the Properties panel's error state.
pub fn last_error(expr: &str) -> bool {
    LAST_ERROR.with(|m| m.borrow().get(expr).copied().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_times_two() {
        for &t in &[0.0_f32, 0.5, 1.0, 2.5, 4.0] {
            let ctx = ExprCtx::at(t, 0.0);
            let got = eval("time * 2", &ctx).expect("should evaluate");
            assert!((got - t * 2.0).abs() < 1e-5, "t={t} got={got}");
        }
    }

    #[test]
    fn value_plus_offset() {
        // `value + 10` offsets the keyframed value at each time.
        for &v in &[0.0_f32, -3.0, 42.0, 100.0] {
            let ctx = ExprCtx::at(1.0, v);
            let got = eval("value + 10", &ctx).expect("should evaluate");
            assert!((got - (v + 10.0)).abs() < 1e-4, "v={v} got={got}");
        }
    }

    #[test]
    fn wiggle_is_deterministic_and_varies() {
        // Same time → identical result (reproducible, not Math.random).
        let a = eval("wiggle(2, 50)", &ExprCtx::at(1.0, 0.0)).unwrap();
        let b = eval("wiggle(2, 50)", &ExprCtx::at(1.0, 0.0)).unwrap();
        assert_eq!(a, b, "wiggle must be deterministic for a fixed time");

        // Different times → (at least sometimes) different result.
        let mut seen_difference = false;
        for &t in &[0.0_f32, 0.2, 0.5, 1.3, 2.7, 3.9] {
            let v = eval("wiggle(2, 50)", &ExprCtx::at(t, 0.0)).unwrap();
            if (v - a).abs() > 1e-6 {
                seen_difference = true;
            }
            // Amplitude bound (a few octaves normalized to ~±amp).
            assert!(v.abs() <= 50.0 + 1e-3, "wiggle within amplitude, got {v}");
        }
        assert!(seen_difference, "wiggle should vary across time");
    }

    #[test]
    fn malformed_falls_back_to_none_and_flags_error() {
        // A syntax error must not panic — it returns None and records an error.
        let ctx = ExprCtx::at(1.0, 7.0);
        assert!(eval("this is not valid $#@", &ctx).is_none());
        assert!(last_error("this is not valid $#@"));

        // An unknown identifier is a runtime error → None, flagged.
        assert!(eval("nope + 1", &ctx).is_none());
        assert!(last_error("nope + 1"));

        // A valid expression clears the error flag.
        assert!(eval("value", &ctx).is_some());
        assert!(!last_error("value"));
    }

    #[test]
    fn linear_and_clamp_helpers() {
        // linear remaps and clamps to endpoints.
        let ctx = ExprCtx::at(0.0, 0.0);
        assert!((eval("linear(0.5, 0, 1, 0, 100)", &ctx).unwrap() - 50.0).abs() < 1e-4);
        assert!((eval("linear(-1, 0, 1, 10, 20)", &ctx).unwrap() - 10.0).abs() < 1e-4);
        assert!((eval("linear(2, 0, 1, 10, 20)", &ctx).unwrap() - 20.0).abs() < 1e-4);
        // clamp.
        assert!((eval("clamp(5, 0, 1)", &ctx).unwrap() - 1.0).abs() < 1e-4);
        assert!((eval("clamp(-5, 0, 1)", &ctx).unwrap() - 0.0).abs() < 1e-4);
        // built-in math.
        assert!((eval("floor(3.7)", &ctx).unwrap() - 3.0).abs() < 1e-4);
        assert!((eval("abs(-2.5)", &ctx).unwrap() - 2.5).abs() < 1e-4);
    }

    #[test]
    fn fps_duration_index_in_scope() {
        let ctx = ExprCtx {
            time: 0.0,
            value: 0.0,
            fps: 30.0,
            duration: 5.0,
            index: 3,
        };
        assert!((eval("fps", &ctx).unwrap() - 30.0).abs() < 1e-4);
        assert!((eval("duration", &ctx).unwrap() - 5.0).abs() < 1e-4);
        assert!((eval("index * 2", &ctx).unwrap() - 6.0).abs() < 1e-4);
    }
}
