//! Spike: 10k variable-height message list in Slint, with frame timing.
//!
//!   slint-list [--count 10000] [--autoscroll-secs 10]
//!
//! Phases: open scrolled to the bottom → auto-scroll up for N seconds while
//! recording frame intervals → jump to the top a few times so `near-top`
//! prepends history → print metrics and exit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use slint::{
    ComponentHandle, Model, ModelRc, RenderingState, SharedString, Timer, TimerMode, VecModel,
};

slint::include_modules!();

const SCROLL_STEP_PX: f32 = 40.0;
const TICK: Duration = Duration::from_millis(16);
const PREPEND_BATCH: usize = 50;
const PREPEND_ROUNDS: usize = 3;

const LATIN: &[&str] = &[
    "hello", "spike", "relay", "network", "message", "about", "tomorrow", "release", "build",
    "check", "the", "and", "with", "works", "fine", "really", "maybe", "link", "photo", "later",
];
const CYRILLIC: &[&str] = &[
    "привет",
    "сообщение",
    "завтра",
    "сеть",
    "работает",
    "проверка",
    "как",
    "дела",
    "отлично",
    "посмотри",
    "ссылка",
    "фото",
    "потом",
    "сборка",
    "релиз",
    "и",
    "в",
    "на",
    "что",
    "это",
];
const EMOJI: &[&str] = &["😀", "👍", "🔥", "🎉", "🤔", "❤️", "🚀"];

struct Args {
    count: usize,
    autoscroll_secs: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        count: 10_000,
        autoscroll_secs: 10,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("{k} needs a value"))?;
        match k.as_str() {
            "--count" => a.count = v.parse().map_err(|_| "bad --count")?,
            "--autoscroll-secs" => {
                a.autoscroll_secs = v.parse().map_err(|_| "bad --autoscroll-secs")?
            }
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    Ok(a)
}

fn gen_message(rng: &mut StdRng, minute: usize) -> Msg {
    let target_len = rng.random_range(1..=600usize);
    let mut text = String::new();
    while text.chars().count() < target_len {
        if !text.is_empty() {
            text.push(' ');
        }
        let word = match rng.random_range(0..20) {
            0 => EMOJI[rng.random_range(0..EMOJI.len())],
            1..=9 => CYRILLIC[rng.random_range(0..CYRILLIC.len())],
            _ => LATIN[rng.random_range(0..LATIN.len())],
        };
        text.push_str(word);
    }
    let text: String = text.chars().take(target_len).collect();
    Msg {
        text: SharedString::from(text),
        outgoing: rng.random_bool(0.5),
        time: SharedString::from(format!("{:02}:{:02}", (minute / 60) % 24, minute % 60)),
        has_photo: rng.random_bool(0.1),
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn private_bytes() -> Option<usize> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    // SAFETY: valid pseudo-handle for this process and a correctly sized,
    // writable counters struct.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast(), c.cb) };
    (ok != 0).then_some(c.PrivateUsage)
}

/// A prepend waiting for the list to re-measure before the anchor is restored.
struct PendingAnchor {
    content_before: f32,
    scroll_before: f32,
    /// Height of the prepended rows, estimated as rows × average row height.
    estimate: f32,
}

struct AnchorShift {
    scroll_before: f32,
    /// scroll-y on the first frame after the insert (Slint does not anchor itself).
    scroll_seen: f32,
    estimate: f32,
    /// Change of ListView's estimated content height across the insert.
    content_delta: f32,
    scroll_after: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Opening,
    AutoScroll,
    Prepend,
    Done,
}

struct Metrics {
    generated: Duration,
    created: Duration,
    first_frame: Option<Duration>,
    frame_times: Vec<Instant>,
    prepends: usize,
    anchor_shifts: Vec<AnchorShift>,
}

fn main() -> Result<(), slint::PlatformError> {
    let launched = Instant::now();
    let args = parse_args().map_err(slint::PlatformError::Other)?;

    let mut rng = StdRng::seed_from_u64(42);
    let base = 100_000; // minutes, so prepended history gets earlier times
    let model = Rc::new(VecModel::from(
        (0..args.count)
            .map(|i| gen_message(&mut rng, base + i))
            .collect::<Vec<_>>(),
    ));
    let generated = launched.elapsed();
    let ui = ListWindow::new()?;
    let created = launched.elapsed();
    ui.set_messages(ModelRc::from(model.clone()));

    let phase = Rc::new(Cell::new(Phase::Opening));
    let metrics = Rc::new(RefCell::new(Metrics {
        generated,
        created,
        first_frame: None,
        frame_times: Vec::new(),
        prepends: 0,
        anchor_shifts: Vec::new(),
    }));
    let prepending = Rc::new(Cell::new(false));

    // Frame timing: only intervals during auto-scroll count.
    let pending: Rc<RefCell<Option<PendingAnchor>>> = Rc::new(RefCell::new(None));
    {
        let metrics = metrics.clone();
        let phase = phase.clone();
        let pending = pending.clone();
        let prepending = prepending.clone();
        let ui_weak = ui.as_weak();
        ui.window()
            .set_rendering_notifier(move |state, _| {
                if !matches!(state, RenderingState::AfterRendering) {
                    return;
                }
                let mut m = metrics.borrow_mut();
                if m.first_frame.is_none() {
                    m.first_frame = Some(launched.elapsed());
                }
                if phase.get() == Phase::AutoScroll {
                    m.frame_times.push(Instant::now());
                }
                // Restore the anchor once the list has re-measured after a prepend.
                let mut slot = pending.borrow_mut();
                let (Some(p), Some(ui)) = (slot.as_mut(), ui_weak.upgrade()) else {
                    return;
                };
                let content = ui.get_content_height();
                let scroll_seen = ui.get_scroll_y();
                let lowest = -(content - ui.get_visible_height()).max(0.0);
                ui.set_scroll_y((p.scroll_before - p.estimate).clamp(lowest, 0.0));
                m.anchor_shifts.push(AnchorShift {
                    scroll_before: p.scroll_before,
                    scroll_seen,
                    estimate: p.estimate,
                    content_delta: content - p.content_before,
                    scroll_after: ui.get_scroll_y(),
                });
                *slot = None;
                prepending.set(false);
            })
            .map_err(|e| slint::PlatformError::Other(format!("rendering notifier: {e:?}")))?;
    }

    // Prepend 50 older messages and keep the anchor in place: shift scroll-y
    // by the growth of the content height.
    {
        let ui_weak = ui.as_weak();
        let model = model.clone();
        let metrics = metrics.clone();
        let prepending = prepending.clone();
        let rng = RefCell::new(StdRng::seed_from_u64(7));
        let pending = pending.clone();
        let phase = phase.clone();
        ui.on_near_top(move || {
            if prepending.get() || phase.get() != Phase::Prepend {
                return;
            }
            let Some(ui) = ui_weak.upgrade() else { return };
            prepending.set(true);
            let content_before = ui.get_content_height();
            let scroll_before = ui.get_scroll_y();
            let first_minute = base.saturating_sub(PREPEND_BATCH * (metrics.borrow().prepends + 1));
            for i in (0..PREPEND_BATCH).rev() {
                model.insert(0, gen_message(&mut rng.borrow_mut(), first_minute + i));
            }
            metrics.borrow_mut().prepends += 1;
            let rows_before = (model.row_count() - PREPEND_BATCH).max(1) as f32;
            let estimate = content_before / rows_before * PREPEND_BATCH as f32;
            *pending.borrow_mut() = Some(PendingAnchor {
                content_before,
                scroll_before,
                estimate,
            });
            ui.window().request_redraw();
        });
    }

    // Driver: open at bottom, auto-scroll, then trigger prepends, then quit.
    let driver = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let phase = phase.clone();
        let metrics = metrics.clone();
        let model = model.clone();
        let prepending = prepending.clone();
        let autoscroll_for = Duration::from_secs(args.autoscroll_secs);
        let mut phase_started = Instant::now();
        let mut rounds = 0;
        let mut last_jump = Instant::now();
        driver.start(TimerMode::Repeated, TICK, move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            match phase.get() {
                Phase::Opening => {
                    if metrics.borrow().first_frame.is_some() {
                        let bottom = -(ui.get_content_height() - ui.get_visible_height()).max(0.0);
                        ui.set_scroll_y(bottom);
                        phase.set(Phase::AutoScroll);
                        phase_started = Instant::now();
                    }
                }
                Phase::AutoScroll => {
                    if phase_started.elapsed() >= autoscroll_for {
                        phase.set(Phase::Prepend);
                    } else {
                        ui.set_scroll_y((ui.get_scroll_y() + SCROLL_STEP_PX).min(0.0));
                    }
                }
                Phase::Prepend => {
                    if rounds >= PREPEND_ROUNDS {
                        if !prepending.get() {
                            phase.set(Phase::Done);
                            report(&metrics.borrow(), model.row_count());
                            let _ = slint::quit_event_loop();
                        }
                    } else if !prepending.get() && last_jump.elapsed() >= Duration::from_millis(500)
                    {
                        ui.set_scroll_y(0.0);
                        rounds += 1;
                        last_jump = Instant::now();
                    }
                }
                Phase::Done => {}
            }
        });
    }

    ui.run()
}

fn report(m: &Metrics, rows: usize) {
    let mut intervals: Vec<f64> = m
        .frame_times
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0)
        .collect();
    intervals.sort_by(|a, b| a.total_cmp(b));
    let p50 = percentile(&intervals, 0.50);
    let p95 = percentile(&intervals, 0.95);
    let max = intervals.last().copied().unwrap_or(0.0);
    let ram_mb = private_bytes().map(|b| b as f64 / (1024.0 * 1024.0));
    println!("slint-list metrics");
    println!("  data generated:   {} ms", m.generated.as_millis());
    println!("  window created:   {} ms", m.created.as_millis());
    println!(
        "  first frame:      {} ms",
        m.first_frame.map_or(0, |d| d.as_millis())
    );
    println!("  frames (scroll):  {}", m.frame_times.len());
    println!("  frame interval:   p50 {p50:.1} ms, p95 {p95:.1} ms, max {max:.1} ms");
    println!("  prepends:         {} (rows now {rows})", m.prepends);
    for a in &m.anchor_shifts {
        println!(
            "    scroll-y {:.0} (next frame {:.0}) -> {:.0} px; estimate {:.0} px; ListView content-height changed {:+.0} px",
            a.scroll_before, a.scroll_seen, a.scroll_after, a.estimate, a.content_delta
        );
    }
    match ram_mb {
        Some(mb) => println!("  private bytes:    {mb:.1} MB"),
        None => println!("  private bytes:    unavailable"),
    }
    let pass = p95 <= 20.0 && max <= 50.0 && ram_mb.is_some_and(|mb| mb < 150.0);
    println!(
        "  automated pass bar (p95<=20, max<=50, RAM<150 MB; no-jump check is manual): {}",
        if pass { "PASS" } else { "FAIL" }
    );
}
