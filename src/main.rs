use ratatui::Terminal;
use crossterm::terminal::{enable_raw_mode, disable_raw_mode};
use serde::{Serialize, Deserialize};
use clap::Parser;
use anyhow::Result;

fn main() -> Result<()> {
    println!("Dependencies are wired up correctly.");
    Ok(())
}
