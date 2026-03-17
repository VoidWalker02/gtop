///This file contains methods for the manipulation of data taken from
///GPUMetrics for the purpose of extracting statistics from them.
use std::collections::VecDeque;
use std::time::Instant;

///The varying data we actually want to do statistics with
///Bundled into a struct.
#[derive(Debug, Clone, Copy)]
pub struct GpuSample {
    pub t: Instant,
    pub util_pct: Option<f32>,
    pub vram_used_mb: Option<u32>,
    pub core_mhz: Option<u32>,
    pub mem_mhz: Option<u32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
}

//Deque of GpuSamples, allows us to project trends and calculate statistics
//Across a span of time.
pub struct GpuHistory {
    cap: usize,
    samples: VecDeque<GpuSample>,
}


impl GpuHistory {
    ///Creates a cap for the deque, right now hardcoded as 120 (60 seconds of data)
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            samples: VecDeque::with_capacity(cap),
        }
    }

    pub fn push(&mut self, s: GpuSample) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(s);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &GpuSample> {
        self.samples.iter()
    }

    // -------- series helpers (for ratatui::Sparkline) --------

    /// Utilization series in 0..=100.
    pub fn util_series_pct_u64(&self) -> Vec<u64> {
        self.samples
            .iter()
            .map(|s| opt_f32_to_u64_pct(s.util_pct))
            .collect()
    }

    /// VRAM used normalized to 0..=100 given total VRAM (in MB).
    /// If total_mb is None/0, returns all zeros.
    pub fn vram_series_norm_0_100(&self, total_mb: Option<u32>) -> Vec<u64> {
        let total = total_mb.unwrap_or(0) as u64;
        if total == 0 {
            return vec![0; self.samples.len()];
        }

        self.samples
            .iter()
            .map(|s| {
                let used = s.vram_used_mb.unwrap_or(0) as u64;
                ratio_to_100(used, total)
            })
            .collect()
    }

    /// Core clock normalized to 0..=100 given max_mhz.
    pub fn core_series_norm_0_100(&self, max_mhz: Option<u32>, fallback_max: u32) -> Vec<u64> {
        // Force the denominator to match the gauge's 3000 MHz for visual parity
        let maxv = 3000;

        self.samples
            .iter()
            .map(|s| ratio_to_100(s.core_mhz.unwrap_or(0) as u64, maxv))
            .collect()
    }

    pub fn first(&self) -> Option<&GpuSample> {
        self.samples.front()
    }

    pub fn last(&self) -> Option<&GpuSample> {
        self.samples.back()
    }


    /// Memory clock normalized to 0..=100 given max_mhz.
    pub fn mem_series_norm_0_100(&self, max_mhz: Option<u32>, fallback_max: u32) -> Vec<u64> {
        let maxv = max_mhz.unwrap_or(fallback_max) as u64;
        if maxv == 0 {
            return vec![0; self.samples.len()];
        }

        self.samples
            .iter()
            .map(|s| ratio_to_100(s.mem_mhz.unwrap_or(0) as u64, maxv))
            .collect()
    }

    /// Temperature normalized to 0..=100 using a fixed ceiling (e.g. 110C).
    pub fn temp_series_norm_0_100(&self, ceiling_c: u64) -> Vec<u64> {
        if ceiling_c == 0 {
            return vec![0; self.samples.len()];
        }

        self.samples
            .iter()
            .map(|s| {
                let t = s.temp_c.unwrap_or(0.0).max(0.0) as u64;
                ratio_to_100(t, ceiling_c)
            })
            .collect()
    }
}

// -------------------------- stats helpers --------------------------

/// Convert an Option<f32> percentage into 0..=100 u64.
pub fn opt_f32_to_u64_pct(v: Option<f32>) -> u64 {
    v.map(|x| x.clamp(0.0, 100.0).round() as u64).unwrap_or(0)
}

/// Compute (num/den)*100 rounded, clamped to 0..=100.
pub fn ratio_to_100(num: u64, den: u64) -> u64 {
    if den == 0 {
        return 0;
    }
    let r = (num as f64 / den as f64) * 100.0;
    (r.round() as i64).clamp(0, 100) as u64
}

/// min/avg/max for a u64 slice
pub fn stats_u64(vals: &[u64]) -> Option<(u64, u64, u64)> {
    if vals.is_empty() {
        return None;
    }
    let min = *vals.iter().min()?;
    let max = *vals.iter().max()?;

    // Use f64 for the average to prevent integer division truncation
    let sum: u64 = vals.iter().sum();
    let avg = (sum as f64 / vals.len() as f64).round() as u64;

    Some((min, avg, max))
}


