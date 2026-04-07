//! Human behavior simulation for anti-detection
//!
//! This module provides realistic input simulation that mimics genuine human
//! browsing patterns, making automated sessions harder to distinguish from
//! real users.
//!
//! - [`MouseSimulator`] — Distance-aware Bézier curve mouse trajectories
//! - [`TypingSimulator`] — Variable-speed typing with natural pauses *(T11)*
//! - [`InteractionSimulator`] — Random scrolls and micro-movements *(T12)*

// All f64→int and int→f64 casts in this module are bounded by construction
// (RNG outputs, clamped durations, step counts ≤ 120) so truncation and sign
// loss cannot occur in practice.  Precision loss from int→f64 is intentional
// for the splitmix64 RNG and Bézier parameter arithmetic.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use crate::error::{BrowserError, Result};

// ─── RNG helpers (splitmix64, no external dep) ────────────────────────────────

/// One splitmix64 step — deterministic, high-quality 64-bit output.
const fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Uniform float in `[0, 1)`.
fn rand_f64(state: &mut u64) -> f64 {
    (splitmix64(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Uniform float in `[min, max)`.
fn rand_range(state: &mut u64, min: f64, max: f64) -> f64 {
    rand_f64(state).mul_add(max - min, min)
}

/// Approximate Gaussian sample via Box–Muller transform.
fn rand_normal(state: &mut u64, mean: f64, std_dev: f64) -> f64 {
    let u1 = rand_f64(state).max(1e-10);
    let u2 = rand_f64(state);
    let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    std_dev.mul_add(z, mean)
}

// ─── Bézier helpers ───────────────────────────────────────────────────────────

fn lerp(p0: (f64, f64), p1: (f64, f64), t: f64) -> (f64, f64) {
    (t.mul_add(p1.0 - p0.0, p0.0), t.mul_add(p1.1 - p0.1, p0.1))
}

/// Evaluate a cubic Bézier curve at parameter `t ∈ [0, 1]`.
fn cubic_bezier(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t: f64,
) -> (f64, f64) {
    let a = lerp(p0, p1, t);
    let b = lerp(p1, p2, t);
    let c = lerp(p2, p3, t);
    lerp(lerp(a, b, t), lerp(b, c, t), t)
}

// ─── MouseSimulator ───────────────────────────────────────────────────────────

/// Simulates human-like mouse movement via distance-aware Bézier curve trajectories.
///
/// Each call to [`move_to`][MouseSimulator::move_to] computes a cubic Bézier path
/// between the current cursor position and the target, then replays it as a sequence
/// of `Input.dispatchMouseEvent` CDP commands with randomised inter-event delays
/// (10–50 ms per segment).  Movement speed naturally slows for long distances and
/// accelerates for short ones — matching human motor-control patterns.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::behavior::MouseSimulator;
///
/// # async fn run(page: &chromiumoxide::Page) -> stygian_browser::Result<()> {
/// let mut mouse = MouseSimulator::new();
/// mouse.move_to(page, 640.0, 400.0).await?;
/// mouse.click(page, 640.0, 400.0).await?;
/// # Ok(())
/// # }
/// ```
pub struct MouseSimulator {
    /// Current cursor X in CSS pixels.
    current_x: f64,
    /// Current cursor Y in CSS pixels.
    current_y: f64,
    /// Splitmix64 RNG state.
    rng: u64,
}

impl Default for MouseSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseSimulator {
    /// Create a simulator seeded from wall-clock time, positioned at (0, 0).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::MouseSimulator;
    /// let mouse = MouseSimulator::new();
    /// assert_eq!(mouse.position(), (0.0, 0.0));
    /// ```
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0x1234_5678_9abc_def0);
        Self {
            current_x: 0.0,
            current_y: 0.0,
            rng: seed,
        }
    }

    /// Create a simulator with a known initial position and deterministic seed.
    ///
    /// Useful for unit-testing path generation without CDP.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::MouseSimulator;
    /// let mouse = MouseSimulator::with_seed_and_position(42, 100.0, 200.0);
    /// assert_eq!(mouse.position(), (100.0, 200.0));
    /// ```
    pub const fn with_seed_and_position(seed: u64, x: f64, y: f64) -> Self {
        Self {
            current_x: x,
            current_y: y,
            rng: seed,
        }
    }

    /// Returns the current cursor position as `(x, y)`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::MouseSimulator;
    /// let mouse = MouseSimulator::new();
    /// let (x, y) = mouse.position();
    /// assert_eq!((x, y), (0.0, 0.0));
    /// ```
    pub const fn position(&self) -> (f64, f64) {
        (self.current_x, self.current_y)
    }

    /// Compute Bézier waypoints for a move from `(from_x, from_y)` to
    /// `(to_x, to_y)`.
    ///
    /// The number of waypoints scales with Euclidean distance — roughly one
    /// point every 8 pixels — with a minimum of 12 and maximum of 120 steps.
    /// Random perpendicular offsets are applied to the two interior control
    /// points to produce natural curved paths.  Each waypoint receives
    /// sub-pixel jitter (±0.8 px) for micro-tremor realism.
    ///
    /// This method is pure (no I/O) and is exposed for testing.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::MouseSimulator;
    /// let mut mouse = MouseSimulator::with_seed_and_position(1, 0.0, 0.0);
    /// let path = mouse.compute_path(0.0, 0.0, 200.0, 0.0);
    /// // always at least 12 steps
    /// assert!(path.len() >= 13);
    /// // starts near origin
    /// assert!((path[0].0).abs() < 5.0);
    /// // ends near target
    /// let last = path[path.len() - 1];
    /// assert!((last.0 - 200.0).abs() < 5.0);
    /// ```
    pub fn compute_path(
        &mut self,
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    ) -> Vec<(f64, f64)> {
        let dx = to_x - from_x;
        let dy = to_y - from_y;
        let distance = dx.hypot(dy);

        // Scale step count with distance; clamp to [12, 120].
        let steps = ((distance / 8.0).round() as usize).clamp(12, 120);

        // Perpendicular unit vector for offsetting control points.
        let (px, py) = if distance > 1.0 {
            (-dy / distance, dx / distance)
        } else {
            (1.0, 0.0)
        };

        // Larger offsets for longer movements (capped at 200 px).
        let offset_scale = (distance * 0.35).min(200.0);
        let cp1_off = rand_normal(&mut self.rng, 0.0, offset_scale * 0.5);
        let cp2_off = rand_normal(&mut self.rng, 0.0, offset_scale * 0.4);

        // Control points at 1/3 and 2/3 of the straight line, offset perp.
        let cp1 = (
            px.mul_add(cp1_off, from_x + dx / 3.0),
            py.mul_add(cp1_off, from_y + dy / 3.0),
        );
        let cp2 = (
            px.mul_add(cp2_off, from_x + 2.0 * dx / 3.0),
            py.mul_add(cp2_off, from_y + 2.0 * dy / 3.0),
        );
        let p0 = (from_x, from_y);
        let p3 = (to_x, to_y);

        (0..=steps)
            .map(|i| {
                let t = i as f64 / steps as f64;
                let (bx, by) = cubic_bezier(p0, cp1, cp2, p3, t);
                // Micro-tremor jitter (± ~0.8 px, normally distributed).
                let jx = rand_normal(&mut self.rng, 0.0, 0.4);
                let jy = rand_normal(&mut self.rng, 0.0, 0.4);
                (bx + jx, by + jy)
            })
            .collect()
    }

    /// Move the cursor to `(to_x, to_y)` using a human-like Bézier trajectory.
    ///
    /// Dispatches `Input.dispatchMouseEvent`(`mouseMoved`) for each waypoint
    /// with randomised 10–50 ms delays.  Updates [`position`][Self::position]
    /// on success.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if any CDP event dispatch fails.
    pub async fn move_to(&mut self, page: &Page, to_x: f64, to_y: f64) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType,
        };

        let path = self.compute_path(self.current_x, self.current_y, to_x, to_y);

        for &(x, y) in &path {
            let params = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseMoved)
                .x(x)
                .y(y)
                .build()
                .map_err(BrowserError::ConfigError)?;

            page.execute(params)
                .await
                .map_err(|e| BrowserError::CdpError {
                    operation: "Input.dispatchMouseEvent(mouseMoved)".to_string(),
                    message: e.to_string(),
                })?;

            let delay_ms = rand_range(&mut self.rng, 10.0, 50.0) as u64;
            sleep(Duration::from_millis(delay_ms)).await;
        }

        self.current_x = to_x;
        self.current_y = to_y;
        Ok(())
    }

    /// Move to `(x, y)` then perform a human-like left-click.
    ///
    /// After arriving at the target the simulator pauses (20–80 ms), sends
    /// `mousePressed`, holds (50–150 ms), then sends `mouseReleased`.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if any CDP event dispatch fails.
    pub async fn click(&mut self, page: &Page, x: f64, y: f64) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };

        self.move_to(page, x, y).await?;

        // Pre-click pause.
        let pre_ms = rand_range(&mut self.rng, 20.0, 80.0) as u64;
        sleep(Duration::from_millis(pre_ms)).await;

        let press = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .x(x)
            .y(y)
            .button(MouseButton::Left)
            .click_count(1i64)
            .build()
            .map_err(BrowserError::ConfigError)?;

        page.execute(press)
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "Input.dispatchMouseEvent(mousePressed)".to_string(),
                message: e.to_string(),
            })?;

        // Hold duration (humans don't click at zero duration).
        let hold_ms = rand_range(&mut self.rng, 50.0, 150.0) as u64;
        sleep(Duration::from_millis(hold_ms)).await;

        let release = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .x(x)
            .y(y)
            .button(MouseButton::Left)
            .click_count(1i64)
            .build()
            .map_err(BrowserError::ConfigError)?;

        page.execute(release)
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "Input.dispatchMouseEvent(mouseReleased)".to_string(),
                message: e.to_string(),
            })?;

        Ok(())
    }
}

// ─── Keyboard helper ─────────────────────────────────────────────────────────

/// Return a plausible adjacent key for typo simulation.
///
/// Looks up `ch` in a basic QWERTY row map and returns a neighbouring key.
/// Non-alphabetic characters fall back to `'x'`.
fn adjacent_key(ch: char, rng: &mut u64) -> char {
    const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
    let lc = ch.to_lowercase().next().unwrap_or(ch);
    for row in ROWS {
        let chars: Vec<char> = row.chars().collect();
        if let Some(idx) = chars.iter().position(|&c| c == lc) {
            let adj = if idx == 0 {
                chars.get(1).copied().unwrap_or(lc)
            } else if idx == chars.len() - 1 || rand_f64(rng) < 0.5 {
                chars.get(idx - 1).copied().unwrap_or(lc)
            } else {
                chars.get(idx + 1).copied().unwrap_or(lc)
            };
            return if ch.is_uppercase() {
                adj.to_uppercase().next().unwrap_or(adj)
            } else {
                adj
            };
        }
    }
    'x'
}

// ─── TypingSimulator ──────────────────────────────────────────────────────────

/// Simulates human-like typing using `Input.dispatchKeyEvent` CDP commands.
///
/// Each character is dispatched as a `keyDown` → `char` → `keyUp` sequence.
/// Capital letters include the Shift modifier mask (`modifiers = 8`).  A
/// configurable error rate causes occasional typos that are corrected via
/// Backspace before the intended character is retyped.  Inter-key delays
/// follow a Gaussian distribution (~80 ms mean, 25 ms σ) clamped to
/// 30–200 ms.
///
/// # Example
///
/// ```no_run
/// # async fn run(page: &chromiumoxide::Page) -> stygian_browser::Result<()> {
/// use stygian_browser::behavior::TypingSimulator;
/// let mut typer = TypingSimulator::new();
/// typer.type_text(page, "Hello, world!").await?;
/// # Ok(())
/// # }
/// ```
pub struct TypingSimulator {
    /// Splitmix64 RNG state.
    rng: u64,
    /// Per-character typo probability (default: 1.5 %).
    error_rate: f64,
}

impl Default for TypingSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypingSimulator {
    /// Create a typing simulator seeded from wall-clock time.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::TypingSimulator;
    /// let typer = TypingSimulator::new();
    /// ```
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0xdead_beef_cafe_babe);
        Self {
            rng: seed,
            error_rate: 0.015,
        }
    }

    /// Create a typing simulator with a fixed seed (useful for testing).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::TypingSimulator;
    /// let typer = TypingSimulator::with_seed(42);
    /// ```
    pub const fn with_seed(seed: u64) -> Self {
        Self {
            rng: seed,
            error_rate: 0.015,
        }
    }

    /// Set the per-character typo probability (clamped to `0.0–1.0`).
    ///
    /// Default is `0.015` (1.5 %).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::TypingSimulator;
    /// let typer = TypingSimulator::new().with_error_rate(0.0);
    /// ```
    #[must_use]
    pub const fn with_error_rate(mut self, rate: f64) -> Self {
        self.error_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Sample a realistic inter-keystroke delay (Gaussian, ~80 ms mean).
    ///
    /// The returned value is clamped to the range 30–200 ms.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::TypingSimulator;
    /// let mut typer = TypingSimulator::with_seed(1);
    /// let delay = typer.keystroke_delay();
    /// assert!(delay.as_millis() >= 30 && delay.as_millis() <= 200);
    /// ```
    pub fn keystroke_delay(&mut self) -> Duration {
        let ms = rand_normal(&mut self.rng, 80.0, 25.0).clamp(30.0, 200.0) as u64;
        Duration::from_millis(ms)
    }

    /// Dispatch one `Input.dispatchKeyEvent` CDP command.
    async fn dispatch_key(
        page: &Page,
        kind: DispatchKeyEventType,
        key: &str,
        text: Option<&str>,
        modifiers: i64,
    ) -> Result<()> {
        let mut b = DispatchKeyEventParams::builder().r#type(kind).key(key);
        if let Some(t) = text {
            b = b.text(t);
        }
        if modifiers != 0 {
            b = b.modifiers(modifiers);
        }
        let params = b.build().map_err(BrowserError::ConfigError)?;
        page.execute(params)
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "Input.dispatchKeyEvent".to_string(),
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Press and release a `Backspace` key (for correcting a typo).
    async fn type_backspace(page: &Page) -> Result<()> {
        Self::dispatch_key(page, DispatchKeyEventType::RawKeyDown, "Backspace", None, 0).await?;
        Self::dispatch_key(page, DispatchKeyEventType::KeyUp, "Backspace", None, 0).await?;
        Ok(())
    }

    /// Send the full `keyDown` → `char` → `keyUp` sequence for one character.
    ///
    /// Capital letters (Unicode uppercase alphabetic) include `modifiers = 8`
    /// (Shift).
    async fn type_char(page: &Page, ch: char) -> Result<()> {
        let text = ch.to_string();
        let modifiers: i64 = if ch.is_uppercase() && ch.is_alphabetic() {
            8
        } else {
            0
        };
        let key = text.as_str();
        Self::dispatch_key(
            page,
            DispatchKeyEventType::KeyDown,
            key,
            Some(&text),
            modifiers,
        )
        .await?;
        Self::dispatch_key(
            page,
            DispatchKeyEventType::Char,
            key,
            Some(&text),
            modifiers,
        )
        .await?;
        Self::dispatch_key(page, DispatchKeyEventType::KeyUp, key, None, modifiers).await?;
        Ok(())
    }

    /// Type `text` into the focused element with human-like keystrokes.
    ///
    /// Each character produces `keyDown` → `char` → `keyUp` events.  With
    /// probability `error_rate` a wrong adjacent key is typed first, then
    /// corrected with Backspace.  Word boundaries (space or newline) receive an
    /// additional 100–400 ms pause to simulate natural word-completion rhythm.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if any CDP call fails.
    pub async fn type_text(&mut self, page: &Page, text: &str) -> Result<()> {
        for ch in text.chars() {
            // Occasionally make a typo: adjacent key → backspace → correct key.
            if rand_f64(&mut self.rng) < self.error_rate {
                let wrong = adjacent_key(ch, &mut self.rng);
                Self::type_char(page, wrong).await?;
                let typo_delay = rand_normal(&mut self.rng, 120.0, 30.0).clamp(60.0, 250.0) as u64;
                sleep(Duration::from_millis(typo_delay)).await;
                Self::type_backspace(page).await?;
                let fix_delay = rand_range(&mut self.rng, 40.0, 120.0) as u64;
                sleep(Duration::from_millis(fix_delay)).await;
            }

            Self::type_char(page, ch).await?;
            sleep(self.keystroke_delay()).await;

            // Extra pause after word boundaries.
            if ch == ' ' || ch == '\n' {
                let word_pause = rand_range(&mut self.rng, 100.0, 400.0) as u64;
                sleep(Duration::from_millis(word_pause)).await;
            }
        }
        Ok(())
    }
}

// ─── InteractionLevel ─────────────────────────────────────────────────────────

/// Intensity level for [`InteractionSimulator`] random interactions.
///
/// # Example
///
/// ```
/// use stygian_browser::behavior::InteractionLevel;
/// assert_eq!(InteractionLevel::default(), InteractionLevel::None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InteractionLevel {
    /// No random interactions are performed.
    #[default]
    None,
    /// Occasional scroll + brief pause (500–1 500 ms).
    Low,
    /// Scroll sequence + mouse wiggle + reading pause (1–3 s).
    Medium,
    /// Full simulation: scrolling, mouse wiggles, hover, and scroll-back.
    High,
}

// ─── InteractionSimulator ─────────────────────────────────────────────────────

/// Simulates random human-like page interactions.
///
/// Combines scroll patterns, mouse micro-movements, and reading pauses to
/// produce convincing human browsing behaviour.  The intensity is controlled
/// by [`InteractionLevel`].
///
/// # Example
///
/// ```no_run
/// # async fn run(page: &chromiumoxide::Page) -> stygian_browser::Result<()> {
/// use stygian_browser::behavior::{InteractionSimulator, InteractionLevel};
/// let mut sim = InteractionSimulator::new(InteractionLevel::Medium);
/// sim.random_interaction(page, 1280.0, 800.0).await?;
/// # Ok(())
/// # }
/// ```
pub struct InteractionSimulator {
    rng: u64,
    mouse: MouseSimulator,
    level: InteractionLevel,
}

impl Default for InteractionSimulator {
    fn default() -> Self {
        Self::new(InteractionLevel::None)
    }
}

impl InteractionSimulator {
    /// Create a new interaction simulator with the given interaction level.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::{InteractionSimulator, InteractionLevel};
    /// let sim = InteractionSimulator::new(InteractionLevel::Low);
    /// ```
    pub fn new(level: InteractionLevel) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0x0123_4567_89ab_cdef);
        Self {
            rng: seed,
            mouse: MouseSimulator::with_seed_and_position(seed ^ 0xca11_ab1e, 400.0, 300.0),
            level,
        }
    }

    /// Create a simulator with a fixed seed (useful for unit-testing).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::{InteractionSimulator, InteractionLevel};
    /// let sim = InteractionSimulator::with_seed(42, InteractionLevel::High);
    /// ```
    pub const fn with_seed(seed: u64, level: InteractionLevel) -> Self {
        Self {
            rng: seed,
            mouse: MouseSimulator::with_seed_and_position(seed ^ 0xca11_ab1e, 400.0, 300.0),
            level,
        }
    }

    /// Evaluate a JavaScript expression on `page`.
    async fn js(page: &Page, expr: String) -> Result<()> {
        page.evaluate(expr)
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "Runtime.evaluate".to_string(),
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Scroll `delta_y` CSS pixels (positive = down, negative = up).
    async fn scroll(page: &Page, delta_y: i64) -> Result<()> {
        Self::js(
            page,
            format!("window.scrollBy({{top:{delta_y},behavior:'smooth'}})"),
        )
        .await
    }

    /// Dispatch synthetic key events to `window` so behavioural-biometric SDKs
    /// (Cloudflare Turnstile Signal Orchestrator, `OpenAI` Sentinel SO) accumulate
    /// non-zero keystroke telemetry before a protected action fires.
    ///
    /// Events are dispatched at window-level (bubbling) since SO listeners are
    /// installed there.  Arrow/Tab keys are used — they do not activate UI
    /// elements but are universally listened for by signal trackers.
    async fn do_keyactivity(&mut self, page: &Page) -> Result<()> {
        const KEYS: &[&str] = &["ArrowDown", "Tab", "ArrowRight", "ArrowUp"];
        let count = 3 + rand_range(&mut self.rng, 0.0, 4.0) as u32;
        for i in 0..count {
            let key = KEYS
                .get((i as usize) % KEYS.len())
                .copied()
                .unwrap_or("Tab");
            let down_delay = rand_range(&mut self.rng, 50.0, 120.0) as u64;
            sleep(Duration::from_millis(down_delay)).await;
            Self::js(
                page,
                format!(
                    "window.dispatchEvent(new KeyboardEvent('keydown',\
                     {{bubbles:true,cancelable:true,key:{key:?},code:{key:?}}}));"
                ),
            )
            .await
            .ok();
            let hold_ms = rand_range(&mut self.rng, 20.0, 60.0) as u64;
            sleep(Duration::from_millis(hold_ms)).await;
            Self::js(
                page,
                format!(
                    "window.dispatchEvent(new KeyboardEvent('keyup',\
                     {{bubbles:true,cancelable:true,key:{key:?},code:{key:?}}}));"
                ),
            )
            .await
            .ok();
        }
        Ok(())
    }

    /// Scroll down a random amount, then partially scroll back up.
    async fn do_scroll(&mut self, page: &Page) -> Result<()> {
        let down = rand_range(&mut self.rng, 200.0, 600.0) as i64;
        Self::scroll(page, down).await?;
        let pause = rand_range(&mut self.rng, 300.0, 1_000.0) as u64;
        sleep(Duration::from_millis(pause)).await;
        let up = -(rand_range(&mut self.rng, 50.0, (down as f64) * 0.4) as i64);
        Self::scroll(page, up).await?;
        Ok(())
    }

    /// Move the mouse to a random point within the viewport.
    async fn do_mouse_wiggle(&mut self, page: &Page, vw: f64, vh: f64) -> Result<()> {
        let tx = rand_range(&mut self.rng, vw * 0.1, vw * 0.9);
        let ty = rand_range(&mut self.rng, vh * 0.1, vh * 0.9);
        self.mouse.move_to(page, tx, ty).await
    }

    /// Perform a random human-like interaction matching the configured level.
    ///
    /// | Level    | Actions                                                   |
    /// | ---------- | ----------------------------------------------------------- |
    /// | `None`   | No-op                                                     |
    /// | `Low`    | One scroll + short pause (500–1 500 ms)                   |
    /// | `Medium` | Scroll + mouse wiggle + reading pause (1–3 s)             |
    /// | `High`   | Medium + second wiggle + optional scroll-back             |
    ///
    /// # Parameters
    ///
    /// - `page` — The active browser page.
    /// - `viewport_w` / `viewport_h` — Approximate viewport size in CSS pixels.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if any CDP call fails.
    pub async fn random_interaction(
        &mut self,
        page: &Page,
        viewport_w: f64,
        viewport_h: f64,
    ) -> Result<()> {
        match self.level {
            InteractionLevel::None => {}
            InteractionLevel::Low => {
                self.do_scroll(page).await?;
                let pause = rand_range(&mut self.rng, 500.0, 1_500.0) as u64;
                sleep(Duration::from_millis(pause)).await;
            }
            InteractionLevel::Medium => {
                self.do_scroll(page).await?;
                let p1 = rand_range(&mut self.rng, 800.0, 2_000.0) as u64;
                sleep(Duration::from_millis(p1)).await;
                // Key events populate behavioural-biometric trackers.
                self.do_keyactivity(page).await?;
                let p2 = rand_range(&mut self.rng, 500.0, 1_500.0) as u64;
                sleep(Duration::from_millis(p2)).await;
                self.do_mouse_wiggle(page, viewport_w, viewport_h).await?;
                let p3 = rand_range(&mut self.rng, 400.0, 1_500.0) as u64;
                sleep(Duration::from_millis(p3)).await;
            }
            InteractionLevel::High => {
                self.do_scroll(page).await?;
                let p1 = rand_range(&mut self.rng, 1_000.0, 5_000.0) as u64;
                sleep(Duration::from_millis(p1)).await;
                self.do_keyactivity(page).await?;
                let p2 = rand_range(&mut self.rng, 400.0, 1_200.0) as u64;
                sleep(Duration::from_millis(p2)).await;
                self.do_mouse_wiggle(page, viewport_w, viewport_h).await?;
                let p3 = rand_range(&mut self.rng, 800.0, 3_000.0) as u64;
                sleep(Duration::from_millis(p3)).await;
                self.do_keyactivity(page).await?;
                let p4 = rand_range(&mut self.rng, 300.0, 800.0) as u64;
                sleep(Duration::from_millis(p4)).await;
                self.do_mouse_wiggle(page, viewport_w, viewport_h).await?;
                let p5 = rand_range(&mut self.rng, 500.0, 2_000.0) as u64;
                sleep(Duration::from_millis(p5)).await;
                // Occasional scroll-back (40 % chance).
                if rand_f64(&mut self.rng) < 0.4 {
                    let up = -(rand_range(&mut self.rng, 50.0, 200.0) as i64);
                    Self::scroll(page, up).await?;
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
        Ok(())
    }
}

// ─── RequestPacer ─────────────────────────────────────────────────────────────

/// Paces programmatic HTTP/CDP requests with human-realistic inter-request delays.
///
/// Prevents tight-loop request patterns that are trivially detectable by server-side
/// rate analysers (Cloudflare, Akamai, `DataDome`, AWS WAF).  Delays follow a truncated
/// Gaussian distribution centred on `mean_ms` with `std_ms` variance, giving natural
/// bursty-but-not-mechanical timing.
///
/// The first call to [`throttle`][RequestPacer::throttle] always returns immediately
/// (no prior request to pace against).
///
/// # Example
///
/// ```no_run
/// use stygian_browser::behavior::RequestPacer;
///
/// # async fn run() {
/// let mut pacer = RequestPacer::new();
/// for url in &["https://a.example.com", "https://b.example.com"] {
///     pacer.throttle().await;
///     // … make request to url …
/// }
/// # }
/// ```
pub struct RequestPacer {
    rng: u64,
    mean_ms: u64,
    std_ms: u64,
    min_ms: u64,
    max_ms: u64,
    last_request: Option<Instant>,
}

impl Default for RequestPacer {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestPacer {
    /// Default pacer: mean 1 200 ms, σ = 400 ms, clamped 400–4 000 ms.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::RequestPacer;
    /// let _pacer = RequestPacer::new();
    /// ```
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0xdead_beef_cafe_1337);
        Self {
            rng: seed,
            mean_ms: 1_200,
            std_ms: 400,
            min_ms: 400,
            max_ms: 4_000,
            last_request: None,
        }
    }

    /// Create with explicit timing parameters (all values in milliseconds).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::RequestPacer;
    /// // Aggressive: ~500 ms mean, σ = 150 ms, clamped 200–1 500 ms.
    /// let _pacer = RequestPacer::with_timing(500, 150, 200, 1_500);
    /// ```
    pub fn with_timing(mean_ms: u64, std_ms: u64, min_ms: u64, max_ms: u64) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0xdead_beef_cafe_1337);
        Self {
            rng: seed,
            mean_ms,
            std_ms,
            min_ms,
            max_ms,
            last_request: None,
        }
    }

    /// Construct from a target requests-per-second rate.
    ///
    /// Mean = `1000 / rps` ms, σ = 25 % of mean, clamped to ±50 % of mean.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::behavior::RequestPacer;
    /// let _pacer = RequestPacer::with_rate(0.5); // ~1 request every 2 s
    /// ```
    pub fn with_rate(requests_per_second: f64) -> Self {
        let mean_ms = (1_000.0 / requests_per_second.max(0.01)) as u64;
        let std_ms = mean_ms / 4;
        let min_ms = mean_ms / 2;
        let max_ms = mean_ms.saturating_mul(2);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() ^ u64::from(d.subsec_nanos()))
            .unwrap_or(0xdead_beef_cafe_1337);
        Self {
            rng: seed,
            mean_ms,
            std_ms,
            min_ms,
            max_ms,
            last_request: None,
        }
    }

    /// Wait until the appropriate inter-request delay has elapsed, then return.
    ///
    /// The first call returns immediately.  Subsequent calls sleep remaining time
    /// to match the sampled target delay.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() {
    /// use stygian_browser::behavior::RequestPacer;
    /// let mut pacer = RequestPacer::new();
    /// pacer.throttle().await; // first call: immediate
    /// pacer.throttle().await; // waits ~1.2 s
    /// # }
    /// ```
    pub async fn throttle(&mut self) {
        let target_ms = rand_normal(&mut self.rng, self.mean_ms as f64, self.std_ms as f64)
            .max(self.min_ms as f64)
            .min(self.max_ms as f64) as u64;

        if let Some(last) = self.last_request {
            let elapsed_ms = last.elapsed().as_millis() as u64;
            if elapsed_ms < target_ms {
                sleep(Duration::from_millis(target_ms - elapsed_ms)).await;
            }
        }
        self.last_request = Some(Instant::now());
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_simulator_starts_at_origin() {
        let mouse = MouseSimulator::new();
        assert_eq!(mouse.position(), (0.0, 0.0));
    }

    #[test]
    fn mouse_simulator_with_seed_and_position() {
        let mouse = MouseSimulator::with_seed_and_position(42, 150.0, 300.0);
        assert_eq!(mouse.position(), (150.0, 300.0));
    }

    #[test]
    fn compute_path_minimum_steps_for_zero_distance() {
        let mut mouse = MouseSimulator::with_seed_and_position(1, 100.0, 100.0);
        let path = mouse.compute_path(100.0, 100.0, 100.0, 100.0);
        // 12 steps minimum => 13 points (0..=12)
        assert!(path.len() >= 13);
    }

    #[test]
    fn compute_path_scales_with_distance() {
        let mut mouse_near = MouseSimulator::with_seed_and_position(1, 0.0, 0.0);
        let mut mouse_far = MouseSimulator::with_seed_and_position(1, 0.0, 0.0);

        let short_path = mouse_near.compute_path(0.0, 0.0, 30.0, 0.0);
        let long_path = mouse_far.compute_path(0.0, 0.0, 800.0, 0.0);

        // Long distance should produce more waypoints.
        assert!(long_path.len() > short_path.len());
    }

    #[test]
    fn compute_path_step_cap_at_120() {
        let mut mouse = MouseSimulator::with_seed_and_position(99, 0.0, 0.0);
        // distance = 10_000 px → would be 1250 steps without cap.
        let path = mouse.compute_path(0.0, 0.0, 10_000.0, 0.0);
        // 120 steps => 121 points
        assert!(path.len() <= 121);
    }

    #[test]
    fn compute_path_endpoint_near_target() {
        let mut mouse = MouseSimulator::with_seed_and_position(7, 0.0, 0.0);
        let target_x = 500.0_f64;
        let target_y = 300.0_f64;
        let path = mouse.compute_path(0.0, 0.0, target_x, target_y);
        let last = path.last().copied().unwrap_or_default();
        // Jitter is tiny; endpoint should be within 5 px.
        assert!(
            (last.0 - target_x).abs() < 5.0,
            "x off by {}",
            (last.0 - target_x).abs()
        );
        assert!(
            (last.1 - target_y).abs() < 5.0,
            "y off by {}",
            (last.1 - target_y).abs()
        );
    }

    #[test]
    fn compute_path_startpoint_near_origin() {
        let mut mouse = MouseSimulator::with_seed_and_position(3, 50.0, 80.0);
        let path = mouse.compute_path(50.0, 80.0, 400.0, 200.0);
        // First point should be close to start.
        if let Some(first) = path.first() {
            assert!((first.0 - 50.0).abs() < 5.0);
            assert!((first.1 - 80.0).abs() < 5.0);
        }
    }

    #[test]
    fn compute_path_diagonal_movement() {
        let mut mouse = MouseSimulator::with_seed_and_position(17, 0.0, 0.0);
        let path = mouse.compute_path(0.0, 0.0, 300.0, 400.0);
        // 500 px distance → ~62 raw steps, clamped to max(12,62).min(120) = 62 → 63 pts
        assert!(path.len() >= 13);
        let last = path.last().copied().unwrap_or_default();
        assert!((last.0 - 300.0).abs() < 5.0);
        assert!((last.1 - 400.0).abs() < 5.0);
    }

    #[test]
    fn compute_path_deterministic_with_same_seed() {
        let mut m1 = MouseSimulator::with_seed_and_position(42, 0.0, 0.0);
        let mut m2 = MouseSimulator::with_seed_and_position(42, 0.0, 0.0);
        let path1 = m1.compute_path(0.0, 0.0, 200.0, 150.0);
        let path2 = m2.compute_path(0.0, 0.0, 200.0, 150.0);
        assert_eq!(path1.len(), path2.len());
        for (a, b) in path1.iter().zip(path2.iter()) {
            assert!((a.0 - b.0).abs() < 1e-9);
            assert!((a.1 - b.1).abs() < 1e-9);
        }
    }

    #[test]
    fn cubic_bezier_at_t0_is_p0() {
        let p0 = (10.0, 20.0);
        let p1 = (50.0, 100.0);
        let p2 = (150.0, 80.0);
        let p3 = (200.0, 30.0);
        let result = cubic_bezier(p0, p1, p2, p3, 0.0);
        assert!((result.0 - p0.0).abs() < 1e-9);
        assert!((result.1 - p0.1).abs() < 1e-9);
    }

    #[test]
    fn cubic_bezier_at_t1_is_p3() {
        let p0 = (10.0, 20.0);
        let p1 = (50.0, 100.0);
        let p2 = (150.0, 80.0);
        let p3 = (200.0, 30.0);
        let result = cubic_bezier(p0, p1, p2, p3, 1.0);
        assert!((result.0 - p3.0).abs() < 1e-9);
        assert!((result.1 - p3.1).abs() < 1e-9);
    }

    #[test]
    fn rand_f64_is_in_unit_interval() {
        let mut state = 12345u64;
        for _ in 0..1000 {
            let v = rand_f64(&mut state);
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn rand_range_stays_in_bounds() {
        let mut state = 99999u64;
        for _ in 0..1000 {
            let v = rand_range(&mut state, 10.0, 50.0);
            assert!((10.0..50.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn typing_simulator_keystroke_delay_is_positive() {
        let mut ts = TypingSimulator::new();
        assert!(ts.keystroke_delay().as_millis() > 0);
    }

    #[test]
    fn typing_simulator_keystroke_delay_in_range() {
        let mut ts = TypingSimulator::with_seed(123);
        for _ in 0..50 {
            let d = ts.keystroke_delay();
            assert!(
                d.as_millis() >= 30 && d.as_millis() <= 200,
                "delay out of range: {}ms",
                d.as_millis()
            );
        }
    }

    #[test]
    fn typing_simulator_error_rate_clamps_to_one() {
        let ts = TypingSimulator::new().with_error_rate(2.0);
        assert!(
            (ts.error_rate - 1.0).abs() < 1e-9,
            "rate should clamp to 1.0"
        );
    }

    #[test]
    fn typing_simulator_error_rate_clamps_to_zero() {
        let ts = TypingSimulator::new().with_error_rate(-0.5);
        assert!(ts.error_rate.abs() < 1e-9, "rate should clamp to 0.0");
    }

    #[test]
    fn typing_simulator_deterministic_with_same_seed() {
        let mut t1 = TypingSimulator::with_seed(999);
        let mut t2 = TypingSimulator::with_seed(999);
        assert_eq!(t1.keystroke_delay(), t2.keystroke_delay());
    }

    #[test]
    fn adjacent_key_returns_different_char() {
        let mut rng = 42u64;
        for &ch in &['a', 'b', 's', 'k', 'z', 'm'] {
            let adj = adjacent_key(ch, &mut rng);
            assert_ne!(adj, ch, "adjacent_key({ch}) should not return itself");
        }
    }

    #[test]
    fn adjacent_key_preserves_case() {
        let mut rng = 7u64;
        let adj = adjacent_key('A', &mut rng);
        assert!(
            adj.is_uppercase(),
            "adjacent_key('A') should return uppercase"
        );
    }

    #[test]
    fn adjacent_key_non_alpha_returns_fallback() {
        let mut rng = 1u64;
        assert_eq!(adjacent_key('!', &mut rng), 'x');
        assert_eq!(adjacent_key('5', &mut rng), 'x');
    }

    #[test]
    fn interaction_level_default_is_none() {
        assert_eq!(InteractionLevel::default(), InteractionLevel::None);
    }

    #[test]
    fn interaction_simulator_with_seed_is_deterministic() {
        let s1 = InteractionSimulator::with_seed(77, InteractionLevel::Low);
        let s2 = InteractionSimulator::with_seed(77, InteractionLevel::Low);
        assert_eq!(s1.rng, s2.rng);
    }

    #[test]
    fn interaction_simulator_default_is_none_level() {
        let sim = InteractionSimulator::default();
        assert_eq!(sim.level, InteractionLevel::None);
    }

    #[test]
    fn request_pacer_new_has_expected_defaults() {
        let p = RequestPacer::new();
        assert_eq!(p.mean_ms, 1_200);
        assert_eq!(p.min_ms, 400);
        assert_eq!(p.max_ms, 4_000);
        assert!(p.last_request.is_none());
    }

    #[test]
    fn request_pacer_with_timing_stores_params() {
        let p = RequestPacer::with_timing(500, 100, 200, 2_000);
        assert_eq!(p.mean_ms, 500);
        assert_eq!(p.std_ms, 100);
        assert_eq!(p.min_ms, 200);
        assert_eq!(p.max_ms, 2_000);
    }

    #[test]
    fn request_pacer_with_rate_computes_mean() {
        // 0.5 rps → mean = 2 000 ms
        let p = RequestPacer::with_rate(0.5);
        assert_eq!(p.mean_ms, 2_000);
        assert_eq!(p.min_ms, 1_000);
        assert_eq!(p.max_ms, 4_000);
    }

    #[test]
    fn request_pacer_with_rate_clamps_extreme() {
        // Very high rps effectively floors to 0.01 rps minimum
        let p = RequestPacer::with_rate(1_000.0);
        assert!(p.mean_ms >= 1);
    }
}
