// Standard IO + time utilities.
//Instant used to track when metrics were last updated.
use std::io;
use std::time::{Duration, Instant};
mod metrics;
use metrics::{GpuMetrics, get_mesa_version};
mod stats;
use stats::{GpuHistory, GpuSample, stats_u64};
use std::collections::VecDeque;
use std::fs::{File,OpenOptions};
use std::io::Write;
use clap::Parser;
use serde::Serialize;


// Crossterm handles terminal input and raw mode.
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

// Ratatui is the TUI library used for layout, widgets, and styling.
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Gauge, Sparkline, Clear, Table, Row, Cell, Wrap},
    style::{Color, Style, Modifier},
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

fn delta_opt_f32(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    Some(b? - a?)
}

fn delta_opt_u32(a: Option<u32>, b: Option<u32>) -> Option<i32> {
    Some(b? as i32 - a? as i32)
}

fn fmt_delta_f32(v: Option<f32>, unit: &str) -> String {
    v.map(|x| format!("{:+.0} {}", x, unit)).unwrap_or_else(|| "--".into())
}

fn fmt_delta_i32(v: Option<i32>, unit: &str) -> String {
    v.map(|x| format!("{:+} {}", x, unit)).unwrap_or_else(|| "--".into())
}


///Formats the value of VRAM into something we can display.
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
fn mhz_ratio(mhz: Option<u32>, max_mhz: u32) -> f64 {
    match mhz {
        Some(m) if max_mhz > 0 => ((m as f64) / (max_mhz as f64)).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

///Reads the contents of the file provided in argument and returns its
///trimmed contents.
fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}


/// Shared gauge coloring logic.
/// Red means “probably bad”, yellow is warning, green is normal.
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

///Introduces a 100 into the sparklines so that it properly
///Displays activity from a ratio of 0-100
fn anchored_100(series: &[u64]) -> Vec<u64> {
    let mut v = Vec::with_capacity(series.len() + 1);
    v.push(100);           // anchor forces max(data)=100
    v.extend_from_slice(series);
    v
}

///Setting up the sparkline graphs
fn render_fixed_sparkline(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    title: &str,
    series_0_100: &[u64],
) {
    let block = Block::default().borders(Borders::ALL).title(title);
    f.render_widget(block.clone(), area);

    let inner = block.inner(area);

    // 1-column gutter on the left to hide the anchor
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let anchored = anchored_100(series_0_100);

    // Render sparkline across full inner (including the gutter column)
    let sp = Sparkline::default().data(&anchored);
    f.render_widget(sp, inner);

    // Clear the gutter column so the 100 anchor doesn't show
    f.render_widget(Clear, cols[0]);
}

///Format the GPU name, so we don't get a massive String.
fn pretty_gpu_name(raw: &str) -> String {
    if let Some(idx) = raw.find("Radeon") {
        raw[idx..].to_string()
    } else {
        raw.replace("Advanced Micro Devices, Inc. ", "")
           .replace("[AMD/ATI] ", "")
    }
}

#[derive(Parser)]
struct Args{
    #[arg(short, long)]
    log: Option<String>,
}

/// Central application state for the gtop TUI.
///
/// `App` owns all runtime state required by the UI layer,
/// including sampled GPU metrics and historical data used
/// for rendering graphs and statistics.
///
/// This struct is mutated on each tick of the event loop.
struct App {
    ///Whether the application should continue running, set to false if user quits
    running: bool,
    ///Tick counter incremented every time data is harvested and UI refreshes.
    tick: u64,
    ///Latest sampled metrics for the detected GPU
    metrics: Vec<GpuMetrics>,
    ///Last 60 seconds of recorded history of relevant metrics.
    hist0: GpuHistory,
    ///Filepath for logging data. 
    log_file: Option<File>
}


/// Creates a new `App` with default runtime state.
    ///
    /// The application starts in a running state with:
    /// - `tick` initialized to `0`
    /// - no sampled GPU metrics
    /// - a history buffer sized for 120 samples
    ///
    /// The history capacity determines how many past
    /// data points are retained for graph rendering.
    
impl App {
    fn new(log_path: Option<String>) -> Self {
        let log_file = log_path.and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });

        Self {
            running: true,
            tick: 0,
            metrics: vec![],
            hist0: GpuHistory::new(120),
            log_file,
        }
    }



///This is the logic that runs on every UI refresh, the bread and butter of the frontend
fn on_tick(&mut self) {
    // 1. Get the hardware metrics first
    //mutable as it changes every tick
    let mut current_metrics = metrics::read_amd_sysfs("card1");

    // 2. Update or preserve the process list
    if self.tick % 4 == 0 {
        // Run the actual scan every ~2 seconds
        //grab list of processes using most amount of VRAM
        current_metrics.processes = metrics::scan_amdgpu_processes();
    } else if let Some(prev) = self.metrics.get(0) {
        // Carry over the list from the last tick so the box stays full
        current_metrics.processes = prev.processes.clone();
    }

    // 3. Store the combined data
    self.metrics = vec![current_metrics.clone()];

    // 4. Push to history (clocks, power, etc.)
    if let Some(gpu) = self.metrics.get(0) {
        self.hist0.push(GpuSample {
            t: Instant::now(),
            util_pct: gpu.utilization_pct,
            vram_used_mb: gpu.vram_used_mb,
            core_mhz: gpu.core_clock_mhz,
            mem_mhz: gpu.mem_clock_mhz,
            temp_c: gpu.temperature_c,
            power_w: gpu.power_w,
        });
    }



    if let Some(ref mut file) = self.log_file {
        if let Ok(json) = serde_json::to_string(&current_metrics) {
            let _ = writeln!(file, "{}", json);
        }
    }

    self.tick += 1;
}
///If user presses q, quit, to add more commands later.
    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.running = false,
            _ => {}
        }
    }
}


/// Entry point for the gtop TUI
///
/// This function:
/// - Enables raw terminal mode
/// - Switches to the alternate screen buffer
/// - Initializes the Crossterm backend
/// - Runs the main application loop
/// - Restores terminal state on exit
///
/// Any error produced by the application loop
/// is propagated to the caller.
///
/// Terminal state is restored before returning,
/// even if the application loop fails.
fn main() -> io::Result<()> {
    let args = Args::parse(); // 1. Parse args here
    
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Pass the log path into run_app
    let res = run_app(&mut terminal, args.log);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}


/// Runs the main gtop event loop.
///
/// This function drives the application lifecycle
/// - Rendering the UI each frame
/// - Checking for keyboard input
/// - Advancing the application state on ticks
///
/// The loop continues until `app.running` becomes `false`.
///
/// A tick occurs whenever no input event is received within
/// `tick_rate`. Ticks are used to trigger metric sampling
/// and update history


fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>, 
    log_path: Option<String> // 3. Accept the path here
) -> io::Result<()> {
    // 4. Initialize App with the path
    let mut app = App::new(log_path);
    let tick_rate = Duration::from_millis(500);

    app.on_tick();

    while app.running {
        terminal.draw(|f| ui(f, &app))?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code);
                }
            }
        } else {
            app.on_tick();
        }
    }

    Ok(())
}




///UI renders the entire text interface for gtop
///It is broken down into the specific blocks of the TUI
///and the information each block contains
fn ui(f: &mut ratatui::Frame, app: &App) {
    let size = f.size();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(3)])
        .split(size);
///top header text
    
    
    let logging_status = if app.log_file.is_some() { "LOGGING ON" } else { "LOGGING OFF" };
    let header_text = format!("gtop — {} — q to quit", logging_status);

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).title("Header"));

    f.render_widget(header, layout[0]);

    let main = Block::default().borders(Borders::ALL).title("GPU Metrics");
    f.render_widget(main.clone(), layout[1]);

    // Inner area inside the main block
    let inner = main.inner(layout[1]);

    // Split the main inner area into:
// -  text area
// -  small gauge area at the bottom
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
let top_row = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
        Constraint::Percentage(60), // left text
        Constraint::Percentage(40), // right graphs/stats
    ])
    .split(inner_chunks[0]);

let left_area = top_row[0];

let left_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(14), // details area height (tune)
        Constraint::Min(0),     // extras fill the rest
    ])
    .split(left_area);

let left_details = left_chunks[0];
let left_extras  = left_chunks[1];

// ===== Extras area =====
let extras_block = Block::default().borders(Borders::ALL).title("Extras");
f.render_widget(extras_block.clone(), left_extras);
let extras_inner = extras_block.inner(left_extras);

let cols = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(extras_inner);

let left_cards = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(cols[0]);

let right_cards = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(cols[1]);

let card_last60   = left_cards[0];
let card_alerts   = left_cards[1];
let card_backend  = right_cards[0];
let card_topology = right_cards[1];

let gpu0 = app.metrics.get(0);
/////////////////////////////////////////

let b = Block::default().borders(Borders::ALL).title("Last 60s (Δ)");
f.render_widget(b.clone(), card_last60);
let area = b.inner(card_last60);

let (du, dv, dc, dm, dt, dp) = if let (Some(first), Some(last)) = (app.hist0.first(), app.hist0.last()) {
    (
        delta_opt_f32(first.util_pct, last.util_pct),
        delta_opt_u32(first.vram_used_mb, last.vram_used_mb),
        delta_opt_u32(first.core_mhz, last.core_mhz),
        delta_opt_u32(first.mem_mhz, last.mem_mhz),
        delta_opt_f32(first.temp_c, last.temp_c),
        delta_opt_f32(first.power_w, last.power_w),
    )
} else {
    (None, None, None, None, None, None)
};

let lines = vec![
    Line::from(format!("Util   {}", fmt_delta_f32(du, "%"))),
    Line::from(format!("VRAM   {}", fmt_delta_i32(dv, "MB"))),
    Line::from(format!("Core   {}", fmt_delta_i32(dc, "MHz"))),
    Line::from(format!("Mem    {}", fmt_delta_i32(dm, "MHz"))),
    Line::from(format!("Temp   {}", fmt_delta_f32(dt, "°C"))),
    Line::from(format!("Power  {}", fmt_delta_f32(dp, "W"))),
];

f.render_widget(Paragraph::new(Text::from(lines)), area);




////////////////

let b = Block::default().borders(Borders::ALL).title("Alerts");
f.render_widget(b.clone(), card_alerts);
let area = b.inner(card_alerts);

let mut lines: Vec<Line> = vec![];

if let Some(g) = gpu0 {
    let vram_r = vram_ratio(g.vram_used_mb, g.vram_total_mb);

    // Temperatures
    if let Some(tj) = g.junction_temp_c {
        if tj >= 105.0 {
            lines.push(Line::from(Span::styled("! Hotspot critical", Style::default().fg(Color::Red))));
        } else if tj >= 95.0 {
            lines.push(Line::from(Span::styled("! Hotspot high", Style::default().fg(Color::Yellow))));
        }
    }

    // Fan Logic
    if let (Some(temp), Some(rpm)) = (g.temperature_c, g.fan_rpm) {
        if temp > 75.0 && rpm == 0 {
            lines.push(Line::from(Span::styled("! Fan Stalled / Passive", Style::default().fg(Color::Red))));
        }
    }

    // PCIe Link Health
     if let Some(pci) = &g.pcie_speed_gen {
            if pci.contains("Gen 1") || pci.contains("Gen 2") {
                lines.push(Line::from(Span::styled("! Low PCIe Bandwidth", Style::default().fg(Color::Yellow))));
            }
        }

    // VRAM
    if vram_r >= 0.90 {
        lines.push(Line::from(Span::styled("! VRAM > 90%", Style::default().fg(Color::Red))));
    } else if vram_r >= 0.80 {
        lines.push(Line::from(Span::styled("! VRAM > 80%", Style::default().fg(Color::Yellow))));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled("✓ All nominal", Style::default().fg(Color::Green))));
    }
} else {
    lines.push(Line::from("--"));
}

f.render_widget(Paragraph::new(Text::from(lines)), area);





//////////////////////////////////

let b = Block::default().borders(Borders::ALL).title("Backend");
f.render_widget(b.clone(), card_backend);
let area = b.inner(card_backend);

let kernel = read_trim("/proc/sys/kernel/osrelease").unwrap_or_else(|| "--".into());
let mesa_ver = get_mesa_version();
let lines = vec![
    Line::from("Source: amdgpu sysfs"),
    Line::from(format!("Kernel: {}", kernel)),
    Line::from(format!("Mesa: {}", mesa_ver)),
    Line::from("Tick: 500ms"),
];

f.render_widget(Paragraph::new(Text::from(lines)), area);


/////////////////
// ===== Topology & Link Status =====
let b = Block::default().borders(Borders::ALL).title("Interface");
f.render_widget(b.clone(), card_topology);
let area = b.inner(card_topology);

let lines = if let Some(gpu) = gpu0 {
    // We'll use 'pcie_gen' instead of 'gen' to avoid the reserved keyword error
    let pcie_str = match (&gpu.pcie_speed_gen, &gpu.pcie_width) {
        (Some(pcie_gen), Some(width)) => format!("{} x{}", pcie_gen, width),
        _ => "--".into(),
    };

    let gtt_str = match (gpu.gtt_used_mb, gpu.gtt_total_mb) {
        (Some(u), Some(t)) => format!("{u} / {t} MB"),
        _ => "--".into(),
    };

   vec![
           Line::from(vec![
               Span::raw("Link:    "),
               Span::styled(pcie_str, Style::default().fg(Color::Cyan)),
           ]),
           Line::from(vec![
               Span::raw("GTT:     "),
               Span::styled(gtt_str, Style::default().fg(Color::Magenta)),
           ]),
           // --- ADD THIS LINE ---
           Line::from(vec![
               Span::raw("Voltage: "),
               Span::styled(
                   gpu.voltage_mv.map(|v| format!("{:.0} mV", v)).unwrap_or("--".into()),
                   Style::default().fg(Color::Yellow),
               ),
           ]),
           Line::from(format!("PCI ID:  {}", gpu.gpu_id)),
       ]
} else {
    vec![Line::from("--")]
};

f.render_widget(Paragraph::new(Text::from(lines)), area);

///////////////////

let right_side = top_row[1];

let right_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(4), // util sparkline
        Constraint::Length(4), // vram sparkline
        Constraint::Length(4), // core sparkline
        Constraint::Length(5), // stats block
        Constraint::Min(0),
    ])
    .split(right_side);





// Text lines
// ===== Left side: GPU details card =====
let gpu0 = app.metrics.get(0);

let details_block = Block::default()
    .borders(Borders::ALL)
    .title("GPU 0");

f.render_widget(details_block.clone(), left_details);

let details_area = details_block.inner(left_details);

// Give the table a little padding so it doesn't hug the border
//let details_area = details_area.inner(&Margin { vertical: 1, horizontal: 1 });

let details_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(2), // GPU name (wrapped)
        Constraint::Min(0),    // metrics table
    ])
    .split(details_area);

if let Some(gpu) = gpu0 {
    let device_name = Paragraph::new(pretty_gpu_name(&gpu.name))
        .wrap(Wrap { trim: true })
        .style(
            Style::default()
                .add_modifier(Modifier::BOLD)
        );

    f.render_widget(device_name, details_chunks[0]);
}




let table = if let Some(gpu) = gpu0 {
    // Pre-format strings once
    let temp_str = gpu.temperature_c.map(|t| format!("{t:.1} °C")).unwrap_or("--".into());
    let junction_str = gpu.junction_temp_c.map(|t| format!("{t:.1} °C")).unwrap_or("--".into());
    let mem_str = gpu.mem_temp_c.map(|t| format!("{t:.1} °C")).unwrap_or("--".into());
    let power_str = gpu.power_w.map(|p| format!("{p:.0} W")).unwrap_or("--".into());
    let fan_str = gpu.fan_rpm.map(|r| format!("{r} RPM")).unwrap_or("--".into());
    let core_str = gpu.core_clock_mhz.map(|c| format!("{c} MHz")).unwrap_or("--".into());
    let memclk_str = gpu.mem_clock_mhz.map(|c| format!("{c} MHz")).unwrap_or("--".into());
    let vram_str = fmt_vram(gpu.vram_used_mb, gpu.vram_total_mb);



    // Rows: Label | Value (with per-row styling on the value cell)
    let rows = vec![
        Row::new(vec![
            Cell::from("Temp"),
            Cell::from(Line::from(Span::styled(temp_str, temp_style(gpu.temperature_c)))),
        ]),
        Row::new(vec![
            Cell::from("Junction"),
            Cell::from(Line::from(Span::styled(junction_str, junction_style(gpu.junction_temp_c)))),
        ]),
        Row::new(vec![
            Cell::from("Mem Temp"),
            Cell::from(Line::from(Span::styled(mem_str, mem_temp_style(gpu.mem_temp_c)))),
        ]),
        Row::new(vec![
            Cell::from("Power"),
            Cell::from(Line::from(Span::styled(power_str, power_style(gpu.power_w)))),
        ]),
        Row::new(vec![
            Cell::from("Fan"),
            Cell::from(fan_str),
        ]),
        Row::new(vec![
            Cell::from("VRAM"),
            Cell::from(vram_str),
        ]),
        Row::new(vec![
            Cell::from("Clocks"),
            Cell::from(format!("core {core_str} | mem {memclk_str}")),
        ]),
    ];

    Table::new(
        rows,
        [
            Constraint::Length(10), // label column
            Constraint::Min(0),     // value column
        ],
    )
    .column_spacing(2)
} else {
    Table::new(
        vec![Row::new(vec![Cell::from("No GPU data"), Cell::from("--")])],
        [Constraint::Length(10), Constraint::Min(0)],
    )
    .column_spacing(2)
};

f.render_widget(table, details_chunks[1]);


// VRAM gauge
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



// Utilization gauge
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

let util_vals = app.hist0.util_series_pct_u64();
let vram_vals = app.hist0.vram_series_norm_0_100(gpu0.and_then(|g| g.vram_total_mb));
let core_vals_history = app.hist0.core_series_norm_0_100(gpu0.and_then(|g| g.max_core_clock_mhz), 3000);

render_fixed_sparkline(f, right_chunks[0], "Util % (60s)", &util_vals);
render_fixed_sparkline(f, right_chunks[1], "VRAM % (60s)", &vram_vals);
render_fixed_sparkline(f, right_chunks[2], "Core % (60s)", &core_vals_history);


let util_stats = stats_u64(&util_vals);
let vram_stats = stats_u64(&vram_vals);
let core_stats = stats_u64(&core_vals_history); // Calculate stats on real data

let stats_lines = vec![
    Line::from(format!(
        "Util   min/avg/max: {} / {} / {}",
        util_stats.map(|s| s.0.to_string()).unwrap_or("--".into()),
        util_stats.map(|s| s.1.to_string()).unwrap_or("--".into()),
        util_stats.map(|s| s.2.to_string()).unwrap_or("--".into()),
    )),
    Line::from(format!(
        "VRAM%  min/avg/max: {} / {} / {}",
        vram_stats.map(|s| s.0.to_string()).unwrap_or("--".into()),
        vram_stats.map(|s| s.1.to_string()).unwrap_or("--".into()),
        vram_stats.map(|s| s.2.to_string()).unwrap_or("--".into()),
    )),
    Line::from(format!(
        "Core%  min/avg/max: {} / {} / {}",
        core_stats.map(|s| s.0.to_string()).unwrap_or("--".into()),
        core_stats.map(|s| s.1.to_string()).unwrap_or("--".into()),
        core_stats.map(|s| s.2.to_string()).unwrap_or("--".into()),
    )),
];

let stats_widget = Paragraph::new(Text::from(stats_lines))
    .block(Block::default().borders(Borders::ALL).title("Stats (60s)"));

f.render_widget(stats_widget, right_chunks[3]);

// ===== NEW: GPU Process List =====
let card_proc = right_chunks[4];
let b = Block::default().borders(Borders::ALL).title("Top GPU Processes");
f.render_widget(b.clone(), card_proc);
let area = b.inner(card_proc);

let rows: Vec<Row> = if let Some(gpu) = gpu0 {
    gpu.processes.iter().take(15).map(|p| {
        Row::new(vec![
            Cell::from(p.pid.to_string()).style(Style::default().fg(Color::DarkGray)), // New PID cell
            Cell::from(p.name.clone()),
            Cell::from(format!("{} MB", p.vram_mb)),
        ])
    }).collect()
} else {
    vec![Row::new(vec![Cell::from("--"), Cell::from("--"), Cell::from("--")])]
};

let table = Table::new(
    rows,
    [
        Constraint::Length(6),  // PID
        Constraint::Min(10),    // Name
        Constraint::Length(10), // VRAM
    ],
)
.header(
    Row::new(vec!["PID", "Process", "VRAM"])
        .style(Style::default().fg(Color::Yellow))
)
.column_spacing(1);

f.render_widget(table, area);


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
