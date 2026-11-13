use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModVariant {
    /// Classic Ultimate SoundTracker (15 samples, no magic tag at 1080, repeat offset in bytes)
    SoundTracker15,
    /// ProTracker / NoiseTracker / FastTracker multi-channel (31 samples, 4-char tag at 1080)
    ProTracker31 { magic: [u8; 4], channels: usize },
}

impl ModVariant {
    pub fn num_samples(self) -> usize {
        match self {
            Self::SoundTracker15 => 15,
            Self::ProTracker31 { .. } => 31,
        }
    }

    pub fn num_channels(self) -> usize {
        match self {
            Self::SoundTracker15 => 4,
            Self::ProTracker31 { channels, .. } => channels,
        }
    }

    pub fn magic_str(self) -> String {
        match self {
            Self::SoundTracker15 => "ST15 (15-Sample)".to_string(),
            Self::ProTracker31 { magic, .. } => String::from_utf8_lossy(&magic).into_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModSample {
    /// 1-based instrument index (1..=15 or 1..=31)
    pub index: usize,
    pub raw_name: [u8; 22],
    pub finetune: u8,
    pub volume: u8,
    /// Loop start offset in bytes
    pub repeat_start: usize,
    /// Loop length in bytes (<= 2 means One-Shot in ProTracker)
    pub repeat_len: usize,
    /// Signed 8-bit PCM samples
    pub data: Vec<i8>,
}

impl ModSample {
    pub fn name(&self) -> String {
        let end = self
            .raw_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.raw_name.len());
        self.raw_name[..end]
            .iter()
            .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    pub fn is_looped(&self) -> bool {
        self.repeat_len > 2 && self.repeat_start < self.data.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SampleUsage {
    pub total_triggers: usize,
    /// Triggers per channel (0-indexed channel -> count)
    pub channel_triggers: Vec<usize>,
    pub min_period: Option<u16>,
    pub max_period: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct ModFile {
    pub raw_title: [u8; 20],
    pub variant: ModVariant,
    pub samples: Vec<ModSample>,
    pub song_len: u8,
    pub restart_pos: u8,
    pub orders: [u8; 128],
    pub num_patterns: usize,
    /// Raw pattern bytes (`num_patterns * 64 * num_channels * 4`) preserved 1:1
    pub pattern_data: Vec<u8>,
    /// Optional trailing bytes after declared sample data in the source file
    pub trailing_bytes: usize,
}

#[derive(Debug)]
pub enum ModError {
    TooShort(usize),
    UnrecognizedFormat,
    TruncatedPatterns { expected: usize, actual: usize },
    SampleTooLong { index: usize, len: usize },
}

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "file too short ({n} bytes) to be a valid .MOD"),
            Self::UnrecognizedFormat => write!(f, "unrecognized .MOD signature / header layout"),
            Self::TruncatedPatterns { expected, actual } => write!(
                f,
                "truncated pattern data (expected at least {expected} bytes, got {actual})"
            ),
            Self::SampleTooLong { index, len } => write!(
                f,
                "sample #{index} exceeds ProTracker 128 KiB word limit ({len} bytes)"
            ),
        }
    }
}

impl std::error::Error for ModError {}

pub fn detect_magic_channels(magic: &[u8; 4]) -> Option<usize> {
    match magic {
        b"M.K." | b"M!K!" | b"M&K!" | b"N.T." | b"FLT4" | b"EXO4" | b"FA04" => Some(4),
        b"FLT8" | b"EXO8" | b"OCTA" | b"OKTA" | b"CD81" | b"FA08" => Some(8),
        b"FA06" => Some(6),
        _ => {
            if &magic[0..3] == b"TDZ" && (b'1'..=b'9').contains(&magic[3]) {
                return Some((magic[3] - b'0') as usize);
            }
            if &magic[1..4] == b"CHN" && (b'1'..=b'9').contains(&magic[0]) {
                return Some((magic[0] - b'0') as usize);
            }
            if (&magic[2..4] == b"CH" || &magic[2..4] == b"CN")
                && magic[0].is_ascii_digit()
                && magic[1].is_ascii_digit()
            {
                let ch = ((magic[0] - b'0') * 10 + (magic[1] - b'0')) as usize;
                if (1..=64).contains(&ch) {
                    return Some(ch);
                }
            }
            None
        }
    }
}

impl ModFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, ModError> {
        if bytes.len() < 600 {
            return Err(ModError::TooShort(bytes.len()));
        }

        let variant = if bytes.len() >= 1084 {
            let mut magic = [0u8; 4];
            magic.copy_from_slice(&bytes[1080..1084]);
            if let Some(channels) = detect_magic_channels(&magic) {
                ModVariant::ProTracker31 { magic, channels }
            } else if looks_like_st15(bytes) {
                ModVariant::SoundTracker15
            } else {
                return Err(ModError::UnrecognizedFormat);
            }
        } else if looks_like_st15(bytes) {
            ModVariant::SoundTracker15
        } else {
            return Err(ModError::UnrecognizedFormat);
        };

        let mut raw_title = [0u8; 20];
        raw_title.copy_from_slice(&bytes[0..20]);

        let num_samples = variant.num_samples();
        let is_st15 = matches!(variant, ModVariant::SoundTracker15);
        let mut cursor = 20usize;
        let mut sample_headers = Vec::with_capacity(num_samples);

        for idx in 1..=num_samples {
            let mut raw_name = [0u8; 22];
            raw_name.copy_from_slice(&bytes[cursor..cursor + 22]);
            let len_words = u16::from_be_bytes([bytes[cursor + 22], bytes[cursor + 23]]) as usize;
            let finetune = bytes[cursor + 24] & 0x0F;
            let volume = bytes[cursor + 25].min(64);
            let rep_start_raw =
                u16::from_be_bytes([bytes[cursor + 26], bytes[cursor + 27]]) as usize;
            let rep_len_words =
                u16::from_be_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;

            let len_bytes = len_words * 2;
            let repeat_start = if is_st15 {
                rep_start_raw
            } else {
                rep_start_raw * 2
            };
            let repeat_len = rep_len_words * 2;

            sample_headers.push((
                idx,
                raw_name,
                finetune,
                volume,
                repeat_start,
                repeat_len,
                len_bytes,
            ));
            cursor += 30;
        }

        let song_len = bytes[cursor];
        let restart_pos = bytes[cursor + 1];
        let mut orders = [0u8; 128];
        orders.copy_from_slice(&bytes[cursor + 2..cursor + 130]);

        let pat_offset = if is_st15 { 600 } else { 1084 };
        let max_pat = orders.iter().copied().max().unwrap_or(0) as usize;
        let num_patterns = max_pat + 1;
        let pat_size = 64 * variant.num_channels() * 4;
        let total_pat_bytes = num_patterns * pat_size;
        let smp_offset = pat_offset + total_pat_bytes;

        if bytes.len() < smp_offset {
            return Err(ModError::TruncatedPatterns {
                expected: smp_offset,
                actual: bytes.len(),
            });
        }

        let pattern_data = bytes[pat_offset..smp_offset].to_vec();

        let mut smp_cursor = smp_offset;
        let mut samples = Vec::with_capacity(num_samples);
        for (idx, raw_name, finetune, volume, repeat_start, repeat_len, len_bytes) in sample_headers
        {
            let avail = bytes.len().saturating_sub(smp_cursor);
            let take = len_bytes.min(avail);
            let mut data = Vec::with_capacity(len_bytes);
            for &b in &bytes[smp_cursor..smp_cursor + take] {
                data.push(b as i8);
            }
            if take < len_bytes {
                data.resize(len_bytes, 0);
            }
            smp_cursor += take;

            samples.push(ModSample {
                index: idx,
                raw_name,
                finetune,
                volume,
                repeat_start,
                repeat_len,
                data,
            });
        }

        let trailing_bytes = bytes.len().saturating_sub(smp_cursor);

        Ok(Self {
            raw_title,
            variant,
            samples,
            song_len,
            restart_pos,
            orders,
            num_patterns,
            pattern_data,
            trailing_bytes,
        })
    }

    pub fn title(&self) -> String {
        let end = self
            .raw_title
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.raw_title.len());
        self.raw_title[..end]
            .iter()
            .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Scans all active Song Orders (plus unreferenced patterns if desired) to determine
    /// which channels trigger each sample and at what period range.
    pub fn analyze_sample_usage(&self) -> Vec<SampleUsage> {
        let num_samples = self.samples.len();
        let num_channels = self.variant.num_channels();
        let mut usages = vec![
            SampleUsage {
                total_triggers: 0,
                channel_triggers: vec![0; num_channels],
                min_period: None,
                max_period: None,
            };
            num_samples + 1
        ];

        let pat_size = 64 * num_channels * 4;
        let active_orders = (self.song_len as usize).clamp(1, 128);
        let mut cur_smp = vec![0usize; num_channels];

        for ord_idx in 0..active_orders {
            let pat = self.orders[ord_idx] as usize;
            if pat >= self.num_patterns {
                continue;
            }
            let p_base = pat * pat_size;
            for row in 0..64 {
                for (ch, cur_ch_smp) in cur_smp.iter_mut().enumerate().take(num_channels) {
                    let c = p_base + (row * num_channels + ch) * 4;
                    let b0 = self.pattern_data[c];
                    let b1 = self.pattern_data[c + 1];
                    let b2 = self.pattern_data[c + 2];

                    let smp = ((b0 & 0xF0) | ((b2 >> 4) & 0x0F)) as usize;
                    let period = (((b0 & 0x0F) as u16) << 8) | (b1 as u16);

                    if smp > 0 && smp <= num_samples {
                        *cur_ch_smp = smp;
                    }
                    if period > 0 {
                        let target_smp = if smp > 0 && smp <= num_samples {
                            smp
                        } else {
                            *cur_ch_smp
                        };
                        if target_smp > 0 && target_smp <= num_samples {
                            let u = &mut usages[target_smp];
                            u.total_triggers += 1;
                            u.channel_triggers[ch] += 1;
                            u.min_period = Some(u.min_period.map_or(period, |m| m.min(period)));
                            u.max_period = Some(u.max_period.map_or(period, |m| m.max(period)));
                        }
                    }
                }
            }
        }

        usages
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ModError> {
        let is_st15 = matches!(self.variant, ModVariant::SoundTracker15);
        let header_size = if is_st15 { 600 } else { 1084 };
        let total_smp_bytes: usize = self
            .samples
            .iter()
            .map(|s| (s.data.len() + 1) & !1)
            .sum();

        let mut out = Vec::with_capacity(header_size + self.pattern_data.len() + total_smp_bytes);
        out.extend_from_slice(&self.raw_title);

        for s in &self.samples {
            let aligned_len = (s.data.len() + 1) & !1;
            let len_words = aligned_len / 2;
            if len_words > u16::MAX as usize {
                return Err(ModError::SampleTooLong {
                    index: s.index,
                    len: aligned_len,
                });
            }

            let rep_start_field = if is_st15 {
                s.repeat_start.min(aligned_len)
            } else {
                (s.repeat_start.min(aligned_len)) / 2
            };
            let rep_len_words = if aligned_len == 0 {
                if s.repeat_len >= 2 { 1 } else { 0 }
            } else {
                ((s.repeat_len.max(2)) / 2).min(len_words.max(1))
            };

            out.extend_from_slice(&s.raw_name);
            out.extend_from_slice(&(len_words as u16).to_be_bytes());
            out.push(s.finetune & 0x0F);
            out.push(s.volume.min(64));
            out.extend_from_slice(&(rep_start_field as u16).to_be_bytes());
            out.extend_from_slice(&(rep_len_words as u16).to_be_bytes());
        }

        out.push(self.song_len);
        out.push(self.restart_pos);
        out.extend_from_slice(&self.orders);

        if let ModVariant::ProTracker31 { magic, .. } = self.variant {
            out.extend_from_slice(&magic);
        }

        debug_assert_eq!(out.len(), header_size);
        out.extend_from_slice(&self.pattern_data);

        for s in &self.samples {
            for &v in &s.data {
                out.push(v as u8);
            }
            if !s.data.len().is_multiple_of(2) {
                out.push(0);
            }
        }

        Ok(out)
    }
}

fn looks_like_st15(bytes: &[u8]) -> bool {
    if bytes.len() < 1624 {
        return false;
    }
    let song_len = bytes[470];
    if !(1..=128).contains(&song_len) {
        return false;
    }
    let orders = &bytes[472..600];
    if orders.iter().any(|&p| p > 63) {
        return false;
    }
    // Check that title and sample names don't contain random high bytes typical of pattern data
    let valid_text = |slice: &[u8]| {
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        slice[..end].iter().all(|&b| (32..=126).contains(&b))
    };
    if !valid_text(&bytes[0..20]) {
        return false;
    }
    for i in 0..15 {
        let o = 20 + i * 30;
        if !valid_text(&bytes[o..o + 22]) {
            return false;
        }
        if bytes[o + 25] > 64 {
            return false;
        }
    }
    true
}
