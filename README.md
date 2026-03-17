# gtop: Linux GPU Resource Monitor

`gtop` is a lightweight, TUI (Terminal User Interface) based GPU monitoring tool specifically designed for **AMD GPUs** on Linux using the `amdgpu` sysfs and `hwmon` interfaces. It provides real-time insights into core clocks, memory usage, temperatures, and per-process VRAM consumption without the overhead of heavy management libraries.

## Features

- **Real-time Metrics**: Monitor GPU utilization, VRAM, Power Draw (Watts), and Fan Speed (RPM).
    
- **Thermal Tracking**: Displays Edge, Junction (Hotspot), and Memory temperatures with color-coded alerts.
    
- **Process Monitoring**: Scans `/proc` to identify which PIDs are consuming VRAM on the `amdgpu` driver.
    
- **Persistent Logging**: Use the `--log` flag to export metrics to a JSON Lines (`.jsonl`) file for long-term analysis.
    
- **Hardware Identity**: Automatically fetches GPU marketing names (e.g., _Radeon RX 9070_) using `udevadm`.
    

## Installation

### Prerequisites

- **Rust**: [Install Rust](https://rustup.rs/) (v1.70+)
    
- **OS**: Linux (requires `/sys/class/drm` and `/sys/kernel/debug`)
    
- **Dependencies**: `pciutils` (for `glxinfo`) and `udev` (for hardware naming).
    

### Build from Source



```
git clone https://github.com/VoidWalker02/gtop.git
cd gtop
cargo build --release
```

##  Usage

Run the live monitor:

```
cargo run
```

Or if you wish to directly run the binary:

```
./target/release/gtop
```

### Logging Data

To capture GPU behavior over time into a machine-readable file:


```
./target/release/gtop --log /path/to/your/session.jsonl
```

### Keybindings

- `q` or `Esc`: Quit the application.