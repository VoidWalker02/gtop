use std::fs;
use std::io;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::process::Command;




// Global cache for GPU identities.
// We only want to run udevadm once per GPU PCI slot, only need the data once
static ID_CACHE: OnceLock<Mutex<HashMap<String, GpuIdentity>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, GpuIdentity>> {
    ID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get_mesa_version() -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg("glxinfo | grep 'OpenGL version' | grep -o 'Mesa [0-9.]*'")
        .output();

    match output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            s.trim().replace("Mesa ", "") // Returns just "25.3.5"
        }
        Err(_) => "--".into(),
    }
}


fn read_first_token(path: impl AsRef<Path>) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.split_whitespace().next().map(|s| s.to_string())
}

#[derive(Debug, Clone)] // Add this line
pub struct GpuProcess {
    pub pid: u32,
    pub name: String,
    pub vram_mb: u64,
}

pub fn scan_amdgpu_processes() -> Vec<GpuProcess> {
    let mut results = Vec::new();

    // Iterate through all PIDs in /proc
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let pid_str = entry.file_name();
            // Only process directories that are numeric PIDs
            let pid = match pid_str.to_str().and_then(|s| s.parse::<u32>().ok()) {
                Some(p) => p,
                None => continue,
            };

            let fdinfo_path = format!("/proc/{}/fdinfo", pid);
            if let Ok(fds) = fs::read_dir(fdinfo_path) {
                for fd in fds.flatten() {
                    // Open the fdinfo file to check for GPU usage
                    if let Ok(file) = fs::File::open(fd.path()) {
                        let reader = BufReader::new(file);
                        let mut is_amdgpu = false;
                        let mut vram_bytes: u64 = 0;

                        for line in reader.lines().flatten() {
                            // Check for the driver identifier
                            if line.contains("drm-driver:") && line.contains("amdgpu") {
                                is_amdgpu = true;
                            }

                            // Extract VRAM usage which is typically in KiB
                            if line.contains("drm-memory-vram:") {
                                let parts: Vec<&str> = line.split_whitespace().collect();
                                // parts[0] is label, parts[1] is the value
                                if let Some(val_str) = parts.get(1) {
                                    if let Ok(val) = val_str.parse::<u64>() {
                                        // If the kernel provides "KiB" unit, convert to bytes
                                        vram_bytes = if parts.get(2) == Some(&"KiB") {
                                            val * 1024
                                        } else {
                                            val
                                        };
                                    }
                                }
                            }
                        }

                        // If this process owns an amdgpu context and uses VRAM, record it
                        if is_amdgpu && vram_bytes > 0 {
                            let name = fs::read_to_string(format!("/proc/{}/comm", pid))
                                .unwrap_or_else(|_| "unknown".into())
                                .trim()
                                .to_string();

                            results.push(GpuProcess {
                                pid,
                                name,
                                vram_mb: vram_bytes / 1024 / 1024,
                            });
                            // One GPU file descriptor is enough to identify the process
                            break;
                        }
                    }
                }
            }
        }
    }

    // Sort descending by VRAM usage so top consumers appear first
    results.sort_by(|a, b| b.vram_mb.cmp(&a.vram_mb));
    results
}


#[derive(Debug, Clone)]
pub struct GpuMetrics {
    pub gpu_id: String, // GPU PCI slot can act as an ID
    pub name: String, //commercial gpu name

    pub temperature_c: Option<f32>,
    pub junction_temp_c: Option<f32>,
    pub mem_temp_c: Option<f32>,

    pub utilization_pct: Option<f32>,

    pub vram_used_mb: Option<u32>,
    pub vram_total_mb: Option<u32>,

    pub power_w: Option<f32>,
    pub fan_rpm: Option<u32>,

    pub core_clock_mhz: Option<u32>,
    pub mem_clock_mhz: Option<u32>,
    pub max_core_clock_mhz: Option<u32>,
    pub max_mem_clock_mhz: Option<u32>,

    pub pcie_speed_gen: Option<String>, // Changed to String for the "Gen X" text
    pub pcie_width: Option<u32>,
    pub gtt_used_mb: Option<u64>,
    pub gtt_total_mb: Option<u64>,

    pub voltage_mv: Option<f32>,

    pub processes: Vec<GpuProcess>, // Add this line

    pub timestamp: Instant,



}

#[derive(Debug, Clone)]
//Collection of data used to identify the current gpu
pub struct GpuIdentity {
    pub pci_slot: String,          // "0000:03:00.0"
    pub vendor: Option<String>,     // pretty string from udev db
    pub model: Option<String>,      // pretty string from udev db
}


/* ------------------------- file helpers ------------------------- */

///Helpers used to help parse the filepaths for the data being sampled from the kernel

fn read_trimmed(path: impl AsRef<Path>) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_string())
}

fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    read_trimmed(path).ok()?.parse::<u64>().ok()
}

fn read_u32(path: impl AsRef<Path>) -> Option<u32> {
    read_trimmed(path).ok()?.parse::<u32>().ok()
}

fn read_f32(path: impl AsRef<Path>) -> Option<f32> {
    read_trimmed(path).ok()?.parse::<f32>().ok()
}

/* ------------------------- locating sysfs bits ------------------------- */

//Locating where all the gpu data is in the guts of sysfs

fn device_dir(card: &str) -> PathBuf {
    PathBuf::from(format!("/sys/class/drm/{card}/device"))
}

fn find_hwmon_dir(card: &str) -> Option<PathBuf> {
    let base = device_dir(card).join("hwmon");

    // Collect once (no iterator cloning needed)
    let mut dirs: Vec<PathBuf> = fs::read_dir(&base).ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    dirs.sort();

    // Prefer the hwmon whose "name" contains "amdgpu"
    for p in &dirs {
        if let Ok(name) = read_trimmed(p.join("name")) {
            if name.to_lowercase().contains("amdgpu") {
                return Some(p.clone());
            }
        }
    }

    // Fallback: first entry
    dirs.into_iter().next()
}




fn read_power_w(hw: &std::path::Path) -> Option<f32> {
    // Different kernels expose different files.
    // On my dGPU (card1) it’s power1_average, on other setups it can be power1_input.
    let candidates = ["power1_average", "power1_input"];

    for name in candidates {
        if let Some(v) = read_f32(hw.join(name)) {
            // amdgpu hwmon power values are typically in microwatts.
            return Some(v / 1_000_000.0);
        }
    }
    None
}

//Harvest the PCI slot name (id)
fn pci_slot_name(card: &str) -> Option<String> {
    let uevent = device_dir(card).join("uevent");
    let text = read_trimmed(uevent).ok()?;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("PCI_SLOT_NAME=") {
            return Some(v.to_string());
        }
    }
    None
}

// Pull vendor/model strings from udev's PCI database
#[cfg(target_os = "linux")]
fn read_udev_identity(pci_slot: &str) -> Option<GpuIdentity> {
    let sys_path = format!("/sys/bus/pci/devices/{pci_slot}");

    let out = Command::new("udevadm")
        .args(["info", "-q", "property", "-p", &sys_path])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let s = String::from_utf8_lossy(&out.stdout);

    let mut vendor = None;
    let mut model = None;

    for line in s.lines() {
        if let Some(v) = line.strip_prefix("ID_VENDOR_FROM_DATABASE=") {
            vendor = Some(v.to_string());
        } else if let Some(m) = line.strip_prefix("ID_MODEL_FROM_DATABASE=") {
            model = Some(m.to_string());
        }
    }

    Some(GpuIdentity {
        pci_slot: pci_slot.to_string(),
        vendor,
        model,
    })
}

// If you compile on macOS/Windows, just skip udev and return None.
#[cfg(not(target_os = "linux"))]
fn read_udev_identity(_pci_slot: &str) -> Option<GpuIdentity> {
    None
}

fn get_identity_cached(pci_slot: &str) -> Option<GpuIdentity> {
    // 1) Try cache first
    if let Ok(guard) = cache().lock() {
        if let Some(id) = guard.get(pci_slot) {
            return Some(id.clone());
        }
    }

    // 2) Not cached, fetch via udevadm
    let id = read_udev_identity(pci_slot)?;

    // 3) Store in cache
    if let Ok(mut guard) = cache().lock() {
        guard.insert(pci_slot.to_string(), id.clone());
    }

    Some(id)
}





/* ------------------------- parsing temps by label ------------------------- */

fn read_hwmon_temps(card: &str) -> (Option<f32>, Option<f32>, Option<f32>) {
    // returns (edge, junction, mem) in °C
    let Some(hw) = find_hwmon_dir(card) else {
        return (None, None, None);
    };

    let mut edge = None;
    let mut junction = None;
    let mut mem = None;

    let entries = match fs::read_dir(&hw) {
        Ok(e) => e,
        Err(_) => return (None, None, None),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };

        if !fname.starts_with("temp") || !fname.ends_with("_label") {
            continue;
        }

        let label = read_trimmed(&path).ok().unwrap_or_default().to_lowercase();
        let input_path = hw.join(fname.replace("_label", "_input"));

        // input is usually millidegrees Celsius
        let c = read_f32(&input_path).map(|v| v / 1000.0);

        match label.as_str() {
            "edge" => edge = c,
            "junction" | "hotspot" => junction = c,
            "mem" | "memory" => mem = c,
            _ => {}
        }
    }

    (edge, junction, mem)
}

/* ------------------------- parsing pp_dpm clocks (current + max) ------------------------- */

fn parse_pp_dpm_current_and_max_mhz(path: impl AsRef<Path>) -> (Option<u32>, Option<u32>) {
    let Ok(text) = read_trimmed(path) else {
        return (None, None);
    };

    let mut current: Option<u32> = None;
    let mut maxv: Option<u32> = None;

    for line in text.lines() {
        // pull out a token like "2200Mhz"
        let mut mhz: Option<u32> = None;
        for tok in line.split_whitespace() {
            if let Some(num) = tok.strip_suffix("Mhz").or_else(|| tok.strip_suffix("MHz")) {
                if let Ok(v) = num.parse::<u32>() {
                    mhz = Some(v);
                    break;
                }
            }
        }
        let Some(v) = mhz else { continue; };

        maxv = Some(maxv.map(|m| m.max(v)).unwrap_or(v));
        if line.contains('*') {
            current = Some(v);
        }
    }

    (current, maxv)
}



/* ------------------------- public entry point ------------------------- */






/// Read metrics for one AMD GPU card (ex: "card1") using sysfs/hwmon.
/// No rocm-smi parsing, no JSON.
pub fn read_amd_sysfs(card: &str) -> GpuMetrics {
    let dev = device_dir(card);

    // stable ID + name
    let pci = pci_slot_name(card).unwrap_or_else(|| card.to_string());

    // Pull pretty name from udev once, then reuse from cache
    let name = if let Some(id) = get_identity_cached(&pci) {
        match (id.vendor, id.model) {
            (Some(v), Some(m)) => format!("{v} — {m}"),
            (_, Some(m)) => m,
            (Some(v), None) => v,
            _ => format!("AMD GPU @ {pci}"),
        }
    } else {
        format!("AMD GPU @ {pci}")
    };

    // util (0–100)
    let utilization_pct = read_f32(dev.join("gpu_busy_percent"));

    // vram (bytes -> MiB)
    let vram_used_mb = read_u64(dev.join("mem_info_vram_used"))
        .map(|b| (b / 1024 / 1024) as u32);
    let vram_total_mb = read_u64(dev.join("mem_info_vram_total"))
        .map(|b| (b / 1024 / 1024) as u32);

    // temps
    let (temperature_c, junction_temp_c, mem_temp_c) = read_hwmon_temps(card);

    // power (µW -> W) and fan (may not exist)
    let mut power_w = None;
    let mut fan_rpm = None;
   if let Some(hw) = find_hwmon_dir(card) {
       power_w = read_power_w(&hw);
       fan_rpm = read_u32(hw.join("fan1_input")); // may still be None if not exposed
   }

    // 1. Read the raw speed (e.g., "16.0") and width (e.g., "16")
    let raw_speed = read_first_token(dev.join("current_link_speed"))
        .and_then(|s| s.parse::<f32>().ok());

    let pcie_width = read_first_token(dev.join("current_link_width"))
        .and_then(|s| s.parse::<u32>().ok());

    let pcie_speed_gen = raw_speed.map(|speed| {
        if speed > 16.0      { "Gen 5".into() }
        else if speed > 8.0  { "Gen 4".into() }
        else if speed > 5.0  { "Gen 3".into() }
        else if speed > 2.5  { "Gen 2".into() }
        else                 { "Gen 1".into() }
});

    // 3. Read GTT stats
    let gtt_used_mb = read_u64(dev.join("mem_info_gtt_used")).map(|b| b / 1024 / 1024);
    let gtt_total_mb = read_u64(dev.join("mem_info_gtt_total")).map(|b| b / 1024 / 1024);

    //Read voltage
    let voltage_mv = if let Some(hw) = find_hwmon_dir(card) {
        read_f32(hw.join("in0_input")) // Returns mV
    } else {
        None
    };

    let processes = scan_amdgpu_processes(); // Or start with vec![] if scanner isn't ready
    // clocks
    let (core_clock_mhz, max_core_clock_mhz) = parse_pp_dpm_current_and_max_mhz(dev.join("pp_dpm_sclk"));
    let (mem_clock_mhz,  max_mem_clock_mhz)  = parse_pp_dpm_current_and_max_mhz(dev.join("pp_dpm_mclk"));

    GpuMetrics {
        gpu_id: pci,
        name,

        temperature_c,
        junction_temp_c,
        mem_temp_c,

        utilization_pct,

        vram_used_mb,
        vram_total_mb,

        power_w,
        fan_rpm,

        core_clock_mhz,
        mem_clock_mhz,
        max_core_clock_mhz,
        max_mem_clock_mhz,

        pcie_speed_gen, // matches the struct field name
        pcie_width,
        gtt_used_mb,
        gtt_total_mb,

        voltage_mv,

        processes,

        timestamp: Instant::now(),
    }
}
