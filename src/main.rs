// Standard IO + time utilities.
// We use Instant mainly to track when metrics were last updated.
use std::io;
use std::time::{Duration, Instant};
mod metrics;
use metrics::GpuMetrics;


// Crossterm handles terminal input + raw mode.
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

// Ratatui is the TUI library we’re using for layout, widgets, and styling.
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Gauge},
    style::{Color, Style},
    Terminal,
};

/// Represents a snapshot of GPU metrics at a given point in time.
/// All fields are Option<T> because not all GPUs or backends
/// expose all metrics reliably.
//#[derive(Debug, Clone)]


/// Helper to format Option<T> values.
/// If a metric isn’t available, show `--` instead of crashing or lying.
fn fmt_opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref().map(|x| x.to_string()).unwrap_or_else(|| "--".into())
}

/// Formats VRAM usage nicely.
fn fmt_vram(used: Option<u32>, total: Option<u32>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{u} / {t} MB"),
        (Some(u), None) => format!("{u} MB / ?"),
        _ => "--".into(),
    }
}

/// Convert VRAM usage into a 0.0–1.0 ratio for the gauge widget.
fn vram_ratio(used: Option<u32>, total: Option<u32>) -> f64 {
    match (used, total) {
        (Some(u), Some(t)) if t > 0 => (u as f64 / t as f64).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Convert a percentage value (0–100) into a gauge ratio.
fn pct_ratio(pct: Option<f32>) -> f64 {
    pct.map(|p| (p.clamp(0.0, 100.0) as f64) / 100.0).unwrap_or(0.0)
}

/// Convert MHz values into a ratio based on a fixed maximum.
/// The max is a placeholder for now and will later come from real GPU data.
fn mhz_ratio(mhz: Option<u32>, max_mhz: u32) -> f64 {
    match mhz {
        Some(m) if max_mhz > 0 => ((m as f64) / (max_mhz as f64)).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Shared gauge coloring logic.
/// Red means “probably bad”, yellow is warning, green is normal.
/// This works well for utilization and VRAM usage.
fn gauge_style(r: f64) -> Style {
    if r >= 0.90 {
        Style::default().fg(Color::Red)
    } else if r >= 0.75 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    }
}

/// Temperature coloring for edge/core temperature.
fn temp_style(temp_c: Option<f32>) -> Style {
    match temp_c {
        Some(t) if t >= 90.0 => Style::default().fg(Color::Red),
        Some(t) if t >= 80.0 => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
        None => Style::default().fg(Color::DarkGray),
    }
}

/// Power draw coloring.
fn power_style(power_w: Option<f32>) -> Style {
    match power_w {
        Some(p) if p >= 300.0 => Style::default().fg(Color::Red),
        Some(p) if p >= 220.0 => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
        None => Style::default().fg(Color::DarkGray),
    }
}

/// Junction (hotspot) runs hotter than edge,
/// so thresholds are intentionally higher.
fn junction_style(temp_c: Option<f32>) -> Style {
    match temp_c {
        Some(t) if t >= 105.0 => Style::default().fg(Color::Red),
        Some(t) if t >= 95.0 => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
        None => Style::default().fg(Color::DarkGray),
    }
}

/// VRAM temperature styling.
fn mem_temp_style(temp_c: Option<f32>) -> Style {
    match temp_c {
        Some(t) if t >= 95.0 => Style::default().fg(Color::Red),
        Some(t) if t >= 85.0 => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
        None => Style::default().fg(Color::DarkGray),
    }
}

/// Fake sampler used during development on macOS.
/// This lets me build and polish the UI before wiring in real Linux data.
fn sample_fake(counter: u64) -> Vec<GpuMetrics> {
    // Fake but realistic-ish values so the UI animates.
    let temp = 45.0 + ((counter % 30) as f32) * 0.3;
    let util = (counter % 100) as f32;
    let used = 1200 + (counter as u32 % 800);
    let total = 16_384;

    let junction = temp + 12.0 + ((counter % 10) as f32) * 0.2;
    let mem_temp = temp + 6.0;

    let core_clk = 800 + (counter as u32 % 1600);
    let mem_clk  = 1000 + (counter as u32 % 800);

    vec![GpuMetrics {
        gpu_id: "mock0".into(),
        name: "AMD Radeon (mock)".to_string(),
        temperature_c: Some(temp),
        junction_temp_c: Some(junction),
        mem_temp_c: Some(mem_temp),
        utilization_pct: Some(util),
        vram_used_mb: Some(used),
        vram_total_mb: Some(total),
        power_w: Some(90.0 + (counter % 20) as f32),
        fan_rpm: Some(1200 + (counter as u32 % 400)),
        core_clock_mhz: Some(core_clk),
        mem_clock_mhz: Some(mem_clk),
        max_core_clock_mhz: Some(3000),
        max_mem_clock_mhz: Some(2500),
        timestamp: Instant::now(),
    }]

}

struct App {
    running: bool,
    tick: u64,
    metrics: Vec<GpuMetrics>,
}


impl App {
    fn new() -> Self {
        Self {
            running: true,
            tick: 0,
            metrics: vec![],
        }
    }

fn on_tick(&mut self) {
    // Use card1 because we confirmed it’s the real dGPU (16GB VRAM).
    self.metrics = vec![metrics::read_amd_sysfs("card1")];
    self.tick += 1;
}

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            _ => {}
        }
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut app = App::new();
    let tick_rate = Duration::from_millis(500);

    // Force first tick so UI isn’t empty
    app.on_tick();

    while app.running {
        terminal.draw(|f| ui(f, &app))?;

        // Input (non-blocking with timeout)
        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code);
                }
            }
        } else {
            // Timeout hit => "tick"
            app.on_tick();
        }
    }

    Ok(())
}

fn ui(f: &mut ratatui::Frame, app: &App) {
    let size = f.size();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(3)])
        .split(size);

    let header_text = format!(
        "gtop — AMD sysfs backend —— q to quit",
    );

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).title("Header"));

    f.render_widget(header, layout[0]);

    let main = Block::default().borders(Borders::ALL).title("GPU Metrics");
    f.render_widget(main.clone(), layout[1]);

    // Inner area inside the main block
    let inner = main.inner(layout[1]);

    // Split the main inner area into:
// - a text area
// - a small gauge area at the bottom
let inner_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),   // text
        Constraint::Length(3),// util
        Constraint::Length(3),// vram
        Constraint::Length(3),// core clock
        Constraint::Length(3) // mem clock
    ])
    .split(inner);


// Text lines (same as before, but remove the VRAM line)
let mut lines: Vec<Line> = vec![];

for (i, gpu) in app.metrics.iter().enumerate() {
    if i > 0 {
        lines.push(Line::from("")); // blank line between GPUs
    }

    lines.push(Line::from(format!("GPU {i}: {}", gpu.name)));
   
    //lines.push(Line::from(format!(
        //"Util: {} %",
        //gpu.utilization_pct.map(|u| format!("{u:.0}")).unwrap_or("--".into())
    //)));


    // Temp line (colored)
let temp_str = gpu.temperature_c.map(|t| format!("{t:.1}")).unwrap_or("--".into());
lines.push(Line::from(vec![
    Span::raw("Temp: "),
    Span::styled(format!("{temp_str} °C"), temp_style(gpu.temperature_c)),
]));

// Junction line (colored)
let junction_str = gpu.junction_temp_c.map(|t| format!("{t:.1}")).unwrap_or("--".into());
lines.push(Line::from(vec![
    Span::raw("Junction: "),
    Span::styled(format!("{junction_str} °C"), junction_style(gpu.junction_temp_c)),
]));

// Mem Temp line (colored)
let mem_str = gpu.mem_temp_c.map(|t| format!("{t:.1}")).unwrap_or("--".into());
lines.push(Line::from(vec![
    Span::raw("Mem Temp: "),
    Span::styled(format!("{mem_str} °C"), mem_temp_style(gpu.mem_temp_c)),
]));


// Power line (colored)
let power_str = gpu.power_w.map(|p| format!("{p:.0}")).unwrap_or("--".into());
lines.push(Line::from(vec![
    Span::raw("Power: "),
    Span::styled(format!("{power_str} W"), power_style(gpu.power_w)),
]));

lines.push(Line::from(format!(
    "Clocks: core {} MHz | mem {} MHz",
    gpu.core_clock_mhz.map(|c| c.to_string()).unwrap_or("--".into()),
    gpu.mem_clock_mhz.map(|c| c.to_string()).unwrap_or("--".into()),
)));

    lines.push(Line::from(format!("Fan: {} RPM", fmt_opt(&gpu.fan_rpm))));
}

let body = Paragraph::new(Text::from(lines));
f.render_widget(body, inner_chunks[0]);

// VRAM gauge (for now: based on GPU 0)
let gpu0 = app.metrics.get(0);
let (ratio, label) = if let Some(gpu) = gpu0 {
    let r = vram_ratio(gpu.vram_used_mb, gpu.vram_total_mb);
    let lbl = match (gpu.vram_used_mb, gpu.vram_total_mb) {
        (Some(u), Some(t)) => format!("VRAM {u} / {t} MB"),
        (Some(u), None) => format!("VRAM {u} / ? MB"),
        _ => "VRAM --".into(),
    };
    (r, lbl)
} else {
    (0.0, "VRAM --".into())
};

let vram_gauge = Gauge::default()
    .block(Block::default().borders(Borders::ALL).title("VRAM Usage"))
    .gauge_style(gauge_style(ratio))
    .ratio(ratio)
    .label(label);



// Utilization gauge (for now: based on GPU 0)
let gpu0 = app.metrics.get(0);
let (util_ratio, util_label) = if let Some(gpu) = gpu0 {
    let r = pct_ratio(gpu.utilization_pct);
    let lbl = gpu
        .utilization_pct
        .map(|u| format!("GPU Util {:.0}%", u))
        .unwrap_or_else(|| "GPU Util --".into());
    (r, lbl)
} else {
    (0.0, "GPU Util --".into())
};

let util_gauge = Gauge::default()
    .block(Block::default().borders(Borders::ALL).title("Utilization"))
    .gauge_style(gauge_style(util_ratio))
    .ratio(util_ratio)
    .label(util_label);


// Clocks gauges (GPU 0)
const CORE_MAX_MHZ: u32 = 3000;
const MEM_MAX_MHZ: u32 = 2500;

let gpu0 = app.metrics.get(0);

let (core_ratio, core_label) = if let Some(gpu) = gpu0 {
    let r = mhz_ratio(gpu.core_clock_mhz, CORE_MAX_MHZ);
    let lbl = gpu
        .core_clock_mhz
        .map(|c| format!("Core {} MHz / {} MHz", c, CORE_MAX_MHZ))
        .unwrap_or_else(|| format!("Core -- / {} MHz", CORE_MAX_MHZ));
    (r, lbl)
} else {
    (0.0, format!("Core -- / {} MHz", CORE_MAX_MHZ))
};

let core_gauge = Gauge::default()
    .block(Block::default().borders(Borders::ALL).title("Core Clock"))
    .gauge_style(gauge_style(core_ratio))
    .ratio(core_ratio)
    .label(core_label);

f.render_widget(core_gauge, inner_chunks[3]);

let (mem_ratio, mem_label) = if let Some(gpu) = gpu0 {
    let r = mhz_ratio(gpu.mem_clock_mhz, MEM_MAX_MHZ);
    let lbl = gpu
        .mem_clock_mhz
        .map(|c| format!("Mem {} MHz / {} MHz", c, MEM_MAX_MHZ))
        .unwrap_or_else(|| format!("Mem -- / {} MHz", MEM_MAX_MHZ));
    (r, lbl)
} else {
    (0.0, format!("Mem -- / {} MHz", MEM_MAX_MHZ))
};

let mem_gauge = Gauge::default()
    .block(Block::default().borders(Borders::ALL).title("Memory Clock"))
    .gauge_style(gauge_style(mem_ratio))
    .ratio(mem_ratio)
    .label(mem_label);

f.render_widget(mem_gauge, inner_chunks[4]);

f.render_widget(util_gauge, inner_chunks[1]);


f.render_widget(vram_gauge, inner_chunks[2]);

    let footer = Paragraph::new(format!("Tick: {}", app.tick))
        .block(Block::default().borders(Borders::ALL).title("Footer"));
    f.render_widget(footer, layout[2]);
}