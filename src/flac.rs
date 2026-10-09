//! FLAC: a decoder for every stream the format allows, and an encoder
//! (fixed and LPC prediction, partitioned Rice coding, stereo decorrelation)
//! whose files any FLAC decoder reads.

/// Decoded audio: one vector of samples per channel.
pub(crate) struct Audio {
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub channels: Vec<Vec<i32>>,
}

/// Why a FLAC stream could not be decoded.
#[derive(Debug)]
pub(crate) struct FlacError(pub String);

type Result<T> = std::result::Result<T, FlacError>;

fn error<T>(message: impl Into<String>) -> Result<T> {
    Err(FlacError(message.into()))
}

// MARK: - Bit reading

struct BitReader<'a> {
    data: &'a [u8],
    /// Next byte to load into the cache.
    pos: usize,
    /// Unread bits, most significant first; the bits below them are zero.
    cache: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        BitReader { data, pos, cache: 0, count: 0 }
    }

    #[inline(always)]
    fn refill(&mut self) {
        if self.count <= 32 && self.pos + 4 <= self.data.len() {
            let word = u32::from_be_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ]);
            self.cache |= (word as u64) << (32 - self.count);
            self.count += 32;
            self.pos += 4;
        }
        while self.count <= 56 && self.pos < self.data.len() {
            self.cache |= (self.data[self.pos] as u64) << (56 - self.count);
            self.count += 8;
            self.pos += 1;
        }
    }

    /// `n` (≤ 32) bits as an unsigned number.
    #[inline(always)]
    fn read(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if self.count < n {
            self.refill();
            if self.count < n {
                return error("unexpected end of stream");
            }
        }
        let value = (self.cache >> (64 - n)) as u32;
        self.cache <<= n;
        self.count -= n;
        Ok(value)
    }

    /// `n` (≤ 32) bits as a two's complement number.
    #[inline(always)]
    fn read_signed(&mut self, n: u32) -> Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        let value = self.read(n)?;
        Ok(((value as i32) << (32 - n)) >> (32 - n))
    }

    /// The number of 0 bits before the next 1 (which is consumed).
    #[inline(always)]
    fn read_unary(&mut self) -> Result<u32> {
        let mut zeros = 0;
        loop {
            if self.count == 0 {
                self.refill();
                if self.count == 0 {
                    return error("unexpected end of stream");
                }
            }
            let leading = self.cache.leading_zeros();
            if leading < self.count {
                zeros += leading;
                // The 1 may be the cache's last bit: a shift by all 64.
                self.cache = self.cache.checked_shl(leading + 1).unwrap_or(0);
                self.count -= leading + 1;
                return Ok(zeros);
            }
            zeros += self.count;
            self.cache = 0;
            self.count = 0;
        }
    }

    /// Drops the bits up to the next byte boundary.
    fn align(&mut self) {
        let drop = self.count % 8;
        self.cache <<= drop;
        self.count -= drop;
    }

    /// Byte offset of the next unread bit (on a byte boundary).
    fn byte_position(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }
}

// MARK: - Checksums

const fn crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC8: [u8; 256] = crc8_table();
static CRC16: [u16; 256] = crc16_table();

fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |crc, &b| CRC8[(crc ^ b) as usize])
}

fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |crc, &b| (crc << 8) ^ CRC16[((crc >> 8) as u8 ^ b) as usize])
}

/// MD5 (RFC 1321), for the STREAMINFO signature of the audio.
struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    filled: usize,
    length: u64,
}

impl Md5 {
    fn new() -> Self {
        Md5 { state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buffer: [0; 64], filled: 0, length: 0 }
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.length += bytes.len() as u64;
        while !bytes.is_empty() {
            let take = (64 - self.filled).min(bytes.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == 64 {
                let block = self.buffer;
                self.block(&block);
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_le_bytes());
        let mut out = [0u8; 16];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn block(&mut self, block: &[u8; 64]) {
        const S: [u32; 64] = [
            7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
            14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10,
            15, 21, 6, 10, 15, 21,
        ];
        const K: [u32; 64] = [
            0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8,
            0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
            0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87,
            0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
            0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039,
            0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
            0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
            0xeb86d391,
        ];
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(S[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// The MD5 of interleaved samples as FLAC signs them (little-endian, the
/// fewest whole bytes that hold `bits`).
fn audio_md5(channels: &[&[i32]], bits: u32) -> [u8; 16] {
    let width = bits.div_ceil(8) as usize;
    let frames = channels.first().map_or(0, |c| c.len());
    let mut md5 = Md5::new();
    let mut chunk = Vec::with_capacity(4096 * channels.len() * width);
    for start in (0..frames).step_by(4096) {
        chunk.clear();
        for i in start..(start + 4096).min(frames) {
            for channel in channels {
                chunk.extend_from_slice(&channel[i].to_le_bytes()[..width]);
            }
        }
        md5.update(&chunk);
    }
    md5.finish()
}

// MARK: - Decoding

/// What a FLAC file's STREAMINFO block says.
pub(crate) struct StreamInfo {
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: u32,
    /// 0 when unknown.
    pub total_samples: u64,
}

/// The STREAMINFO of a FLAC file (only its start is needed).
pub(crate) fn stream_info(data: &[u8]) -> Result<StreamInfo> {
    read_metadata(data).map(|(info, _)| info)
}

/// The STREAMINFO and where the first frame starts.
fn read_metadata(data: &[u8]) -> Result<(StreamInfo, usize)> {
    let mut pos = 0;
    // An ID3v2 tag may come first.
    if data.len() >= 10 && &data[0..3] == b"ID3" {
        let size = data[6..10].iter().fold(0usize, |s, &b| (s << 7) | (b & 0x7f) as usize);
        pos = 10 + size + if data[5] & 0x10 != 0 { 10 } else { 0 };
    }
    if data.get(pos..pos + 4) != Some(b"fLaC") {
        return error("not a FLAC file");
    }
    pos += 4;
    let mut info = None;
    loop {
        let Some(header) = data.get(pos..pos + 4) else { return error("truncated metadata") };
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7f;
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        pos += 4;
        if kind == 0 {
            let Some(block) = data.get(pos..pos + length.max(34)) else { return error("truncated STREAMINFO") };
            let mut r = BitReader::new(block, 10);
            let sample_rate = r.read(20)?;
            let channels = r.read(3)? as usize + 1;
            let bits_per_sample = r.read(5)? + 1;
            let total_samples = ((r.read(4)? as u64) << 32) | r.read(32)? as u64;
            info = Some(StreamInfo { sample_rate, channels, bits_per_sample, total_samples });
        }
        pos += length;
        if last {
            break;
        }
        // Only the start of a file may be at hand: STREAMINFO comes first.
        if info.is_some() && pos + 4 > data.len() {
            break;
        }
    }
    let Some(info) = info else { return error("no STREAMINFO") };
    if info.bits_per_sample < 4 {
        return error("unsupported sample size");
    }
    Ok((info, pos))
}

/// Decodes a FLAC file.
pub(crate) fn decode(data: &[u8]) -> Result<Audio> {
    let (info, mut pos) = read_metadata(data)?;
    let capacity = info.total_samples.min(1 << 31) as usize;
    let mut channels: Vec<Vec<i32>> = (0..info.channels).map(|_| Vec::with_capacity(capacity)).collect();
    let mut sample_rate = info.sample_rate;
    let mut scratch: Vec<Vec<i64>> = vec![Vec::new(); 8];
    while pos + 2 <= data.len() {
        if data[pos] != 0xff || data[pos + 1] & 0xfe != 0xf8 {
            // Not a frame here: look for the next one (or stop at trailing tags).
            match data[pos + 1..].windows(2).position(|w| w[0] == 0xff && w[1] & 0xfe == 0xf8) {
                Some(skip) => {
                    pos += 1 + skip;
                    continue;
                }
                None => break,
            }
        }
        match decode_frame(data, pos, &info, &mut scratch) {
            Ok((next, rate, frame_channels, block)) => {
                if frame_channels != info.channels {
                    return error("a frame changes the channel count");
                }
                if rate != 0 {
                    sample_rate = rate;
                }
                for (out, decoded) in channels.iter_mut().zip(&scratch) {
                    out.extend(decoded[..block].iter().map(|&s| s as i32));
                }
                pos = next;
            }
            // A false sync code (or a damaged frame): resynchronize.
            Err(_) => pos += 1,
        }
    }
    if info.total_samples != 0 && (channels[0].len() as u64) < info.total_samples {
        return error(format!("{} of {} samples decoded", channels[0].len(), info.total_samples));
    }
    if info.total_samples != 0 {
        for channel in &mut channels {
            channel.truncate(info.total_samples as usize);
        }
    }
    if sample_rate == 0 {
        return error("no sample rate");
    }
    Ok(Audio { sample_rate, bits_per_sample: info.bits_per_sample, channels })
}

/// Decodes the frame at `start` into `out` (one vector per channel).
/// Returns the position after it, its sample rate (0: the stream's),
/// channel count and block size.
fn decode_frame(
    data: &[u8],
    start: usize,
    info: &StreamInfo,
    out: &mut [Vec<i64>],
) -> Result<(usize, u32, usize, usize)> {
    let mut r = BitReader::new(data, start);
    r.read(15)?; // sync code and a reserved bit
    r.read(1)?; // blocking strategy
    let size_code = r.read(4)?;
    let rate_code = r.read(4)?;
    let assignment = r.read(4)?;
    let bits_code = r.read(3)?;
    if r.read(1)? != 0 {
        return error("reserved bit set");
    }
    // The frame or sample number, UTF-8-like coded.
    let first = r.read(8)?;
    let extra = match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        0xf8..=0xfb => 4,
        0xfc..=0xfd => 5,
        0xfe => 6,
        _ => return error("bad frame number"),
    };
    for _ in 0..extra {
        if r.read(8)? & 0xc0 != 0x80 {
            return error("bad frame number");
        }
    }
    let block = match size_code {
        0 => return error("reserved block size"),
        1 => 192,
        2..=5 => 576 << (size_code - 2),
        6 => r.read(8)? as usize + 1,
        7 => r.read(16)? as usize + 1,
        _ => 256 << (size_code - 8),
    };
    let rate = match rate_code {
        0 => 0,
        1 => 88200,
        2 => 176400,
        3 => 192000,
        4 => 8000,
        5 => 16000,
        6 => 22050,
        7 => 24000,
        8 => 32000,
        9 => 44100,
        10 => 48000,
        11 => 96000,
        12 => r.read(8)? * 1000,
        13 => r.read(16)?,
        14 => r.read(16)? * 10,
        _ => return error("bad sample rate"),
    };
    let bits = match bits_code {
        0 => info.bits_per_sample,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return error("reserved sample size"),
    };
    let header_end = r.byte_position();
    let crc = r.read(8)? as u8;
    if crc8(&data[start..header_end]) != crc {
        return error("header CRC mismatch");
    }
    let channel_count = match assignment {
        0..=7 => assignment as usize + 1,
        8..=10 => 2,
        _ => return error("reserved channel assignment"),
    };
    if channel_count > out.len() {
        return error("too many channels");
    }
    for (channel, samples) in out.iter_mut().enumerate().take(channel_count) {
        // The side channel has one more bit.
        let side = matches!((assignment, channel), (8, 1) | (9, 0) | (10, 1));
        samples.clear();
        samples.resize(block, 0);
        decode_subframe(&mut r, bits + side as u32, samples)?;
    }
    match assignment {
        8 => {
            let (left, side) = out.split_at_mut(1);
            for (l, s) in left[0].iter().zip(side[0].iter_mut()) {
                *s = l.wrapping_sub(*s);
            }
        }
        9 => {
            let (side, right) = out.split_at_mut(1);
            for (s, r) in side[0].iter_mut().zip(right[0].iter()) {
                *s = s.wrapping_add(*r);
            }
        }
        10 => {
            let (mid, side) = out.split_at_mut(1);
            for (m, s) in mid[0].iter_mut().zip(side[0].iter_mut()) {
                let full = (*m << 1) | (*s & 1);
                let left = full.wrapping_add(*s) >> 1;
                let right = full.wrapping_sub(*s) >> 1;
                *m = left;
                *s = right;
            }
        }
        _ => {}
    }
    r.align();
    let frame_end = r.byte_position();
    let crc = r.read(16)? as u16;
    if crc16(&data[start..frame_end]) != crc {
        return error("frame CRC mismatch");
    }
    Ok((r.byte_position(), rate, channel_count, block))
}

fn decode_subframe(r: &mut BitReader<'_>, bits: u32, out: &mut [i64]) -> Result<()> {
    if r.read(1)? != 0 {
        return error("bad subframe padding");
    }
    let kind = r.read(6)?;
    let wasted = if r.read(1)? != 0 { r.read_unary()? + 1 } else { 0 };
    if wasted >= bits {
        return error("bad wasted bits");
    }
    let bits = bits - wasted;
    let n = out.len();
    match kind {
        0 => {
            let value = read_wide(r, bits)?;
            out.fill(value);
        }
        1 => {
            for sample in out.iter_mut() {
                *sample = read_wide(r, bits)?;
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > n {
                return error("predictor order beyond the block");
            }
            for sample in out[..order].iter_mut() {
                *sample = read_wide(r, bits)?;
            }
            read_residual(r, order, out)?;
            restore_fixed(order, out);
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > n {
                return error("predictor order beyond the block");
            }
            for sample in out[..order].iter_mut() {
                *sample = read_wide(r, bits)?;
            }
            let precision = r.read(4)? + 1;
            if precision == 16 {
                return error("bad coefficient precision");
            }
            let shift = r.read_signed(5)?;
            if shift < 0 {
                return error("negative LPC shift");
            }
            let mut coefficients = [0i64; 32];
            for c in coefficients[..order].iter_mut() {
                *c = r.read_signed(precision)? as i64;
            }
            read_residual(r, order, out)?;
            restore_lpc(&coefficients[..order], shift as u32, out);
        }
        _ => return error("reserved subframe type"),
    }
    if wasted > 0 {
        for sample in out.iter_mut() {
            *sample <<= wasted;
        }
    }
    Ok(())
}

/// A signed sample of up to 33 bits (a 32-bit side channel).
fn read_wide(r: &mut BitReader<'_>, bits: u32) -> Result<i64> {
    if bits <= 32 {
        Ok(r.read_signed(bits)? as i64)
    } else {
        let high = r.read_signed(bits - 32)? as i64;
        Ok((high << 32) | r.read(32)? as i64)
    }
}

/// The residual, into `out[order..]`.
fn read_residual(r: &mut BitReader<'_>, order: usize, out: &mut [i64]) -> Result<()> {
    let method = r.read(2)?;
    let (parameter_bits, escape) = match method {
        0 => (4, 15),
        1 => (5, 31),
        _ => return error("reserved residual coding"),
    };
    let partition_order = r.read(4)?;
    let partitions = 1usize << partition_order;
    let n = out.len();
    if n % partitions != 0 || n / partitions < order {
        return error("bad partition order");
    }
    let mut at = order;
    for partition in 0..partitions {
        let count = n / partitions - if partition == 0 { order } else { 0 };
        let parameter = r.read(parameter_bits)?;
        let slice = &mut out[at..at + count];
        if parameter == escape {
            let raw = r.read(5)?;
            for sample in slice.iter_mut() {
                *sample = r.read_signed(raw)? as i64;
            }
        } else {
            for sample in slice.iter_mut() {
                let high = r.read_unary()?;
                let low = r.read(parameter)?;
                let folded = ((high as u64) << parameter) | low as u64;
                *sample = (folded >> 1) as i64 ^ -((folded & 1) as i64);
            }
        }
        at += count;
    }
    Ok(())
}

fn restore_fixed(order: usize, x: &mut [i64]) {
    // Wrapping: a damaged frame (caught by its CRC afterwards) must not
    // overflow; a valid one never comes near.
    for i in order..x.len() {
        let prediction = match order {
            0 => 0,
            1 => x[i - 1],
            2 => x[i - 1].wrapping_mul(2).wrapping_sub(x[i - 2]),
            3 => x[i - 1].wrapping_sub(x[i - 2]).wrapping_mul(3).wrapping_add(x[i - 3]),
            _ => x[i - 1]
                .wrapping_add(x[i - 3])
                .wrapping_mul(4)
                .wrapping_sub(x[i - 2].wrapping_mul(6))
                .wrapping_sub(x[i - 4]),
        };
        x[i] = x[i].wrapping_add(prediction);
    }
}

fn restore_lpc(coefficients: &[i64], shift: u32, x: &mut [i64]) {
    let order = coefficients.len();
    for i in order..x.len() {
        let mut sum = 0i64;
        for (j, &c) in coefficients.iter().enumerate() {
            sum = sum.wrapping_add(c.wrapping_mul(x[i - 1 - j]));
        }
        x[i] = x[i].wrapping_add(sum >> shift);
    }
}

// MARK: - Encoding

struct BitWriter {
    bytes: Vec<u8>,
    accumulator: u64,
    count: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { bytes: Vec::new(), accumulator: 0, count: 0 }
    }

    /// Writes the low `n` (≤ 32) bits of `value`.
    #[inline(always)]
    fn write(&mut self, n: u32, value: u64) {
        if n == 0 {
            return;
        }
        let value = value & ((1u64 << n) - 1);
        self.accumulator = (self.accumulator << n) | value;
        self.count += n;
        while self.count >= 8 {
            self.count -= 8;
            self.bytes.push((self.accumulator >> self.count) as u8);
        }
    }

    fn write_signed(&mut self, n: u32, value: i64) {
        self.write(n, value as u64);
    }

    fn write_unary(&mut self, zeros: u32) {
        let mut left = zeros;
        while left >= 32 {
            self.write(32, 0);
            left -= 32;
        }
        self.write(left + 1, 1);
    }

    fn align(&mut self) {
        if self.count > 0 {
            self.write(8 - self.count, 0);
        }
    }
}

/// Samples per frame.
const BLOCK: usize = 4096;
const MAX_LPC_ORDER: usize = 12;
const MAX_PARTITION_ORDER: u32 = 8;

/// Encodes `channels` (1…8, equally long) of `bits_per_sample` (4…24)
/// samples as a FLAC file.
pub(crate) fn encode(channels: &[&[i32]], sample_rate: u32, bits_per_sample: u32) -> Vec<u8> {
    assert!((1..=8).contains(&channels.len()), "1…8 channels");
    assert!((4..=24).contains(&bits_per_sample), "4…24 bits");
    let frames = channels[0].len();
    assert!(channels.iter().all(|c| c.len() == frames), "channels of one length");

    let mut stream = BitWriter::new();
    let mut min_frame = u32::MAX;
    let mut max_frame = 0u32;
    let mut number = 0u64;
    for start in (0..frames).step_by(BLOCK) {
        let end = (start + BLOCK).min(frames);
        let blocks: Vec<&[i32]> = channels.iter().map(|c| &c[start..end]).collect();
        let before = stream.bytes.len();
        encode_frame(&mut stream, &blocks, number, bits_per_sample);
        let size = (stream.bytes.len() - before) as u32;
        min_frame = min_frame.min(size);
        max_frame = max_frame.max(size);
        number += 1;
    }

    let mut out = Vec::with_capacity(stream.bytes.len() + 42);
    out.extend_from_slice(b"fLaC");
    out.push(0x80); // the last metadata block: STREAMINFO
    out.extend_from_slice(&[0, 0, 34]);
    let mut info = BitWriter::new();
    let block = BLOCK.min(frames.max(16)) as u64;
    info.write(16, block);
    info.write(16, block);
    info.write(24, if number == 0 { 0 } else { min_frame as u64 });
    info.write(24, max_frame as u64);
    info.write(20, sample_rate as u64);
    info.write(3, channels.len() as u64 - 1);
    info.write(5, bits_per_sample as u64 - 1);
    info.write(4, (frames as u64) >> 32);
    info.write(32, frames as u64 & 0xffff_ffff);
    out.extend_from_slice(&info.bytes);
    out.extend_from_slice(&audio_md5(channels, bits_per_sample));
    out.extend_from_slice(&stream.bytes);
    out
}

fn encode_frame(w: &mut BitWriter, blocks: &[&[i32]], number: u64, bits: u32) {
    let n = blocks[0].len();
    let start = w.bytes.len();
    debug_assert_eq!(w.count, 0);

    // Stereo: the cheapest of left/right, left/side, side/right and mid/side.
    let assignment;
    let subframes: Vec<BitWriter>;
    if blocks.len() == 2 {
        let left: Vec<i64> = blocks[0].iter().map(|&s| s as i64).collect();
        let right: Vec<i64> = blocks[1].iter().map(|&s| s as i64).collect();
        let side: Vec<i64> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        let mid: Vec<i64> = left.iter().zip(&right).map(|(l, r)| (l + r) >> 1).collect();
        let l = encode_subframe(&left, bits);
        let r = encode_subframe(&right, bits);
        let s = encode_subframe(&side, bits + 1);
        let m = encode_subframe(&mid, bits);
        let size =
            |a: &BitWriter, b: &BitWriter| (a.bytes.len() * 8) as u32 + a.count + (b.bytes.len() * 8) as u32 + b.count;
        let options = [(1, size(&l, &r)), (8, size(&l, &s)), (9, size(&s, &r)), (10, size(&m, &s))];
        let best = options.iter().min_by_key(|o| o.1).unwrap().0;
        assignment = best;
        subframes = match best {
            1 => vec![l, r],
            8 => vec![l, s],
            9 => vec![s, r],
            _ => vec![m, s],
        };
    } else {
        assignment = blocks.len() as u64 - 1;
        subframes =
            blocks.iter().map(|b| encode_subframe(&b.iter().map(|&s| s as i64).collect::<Vec<_>>(), bits)).collect();
    }

    // Header.
    w.write(15, 0x7ffc); // sync code, reserved 0
    w.write(1, 0); // fixed block size
    let size_code = if n == BLOCK { 12 } else { 7 };
    w.write(4, size_code);
    w.write(4, 0); // sample rate from STREAMINFO
    w.write(4, assignment);
    let bits_code = match bits {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        _ => 0,
    };
    w.write(3, bits_code);
    w.write(1, 0);
    write_utf8(w, number);
    if size_code == 7 {
        w.write(16, n as u64 - 1);
    }
    let crc = crc8(&w.bytes[start..]);
    w.write(8, crc as u64);

    for subframe in &subframes {
        append(w, subframe);
    }
    w.align();
    let crc = crc16(&w.bytes[start..]);
    w.write(16, crc as u64);
}

/// Appends the bits of `from` to `w`.
fn append(w: &mut BitWriter, from: &BitWriter) {
    if w.count == 0 {
        w.bytes.extend_from_slice(&from.bytes);
    } else {
        for &b in &from.bytes {
            w.write(8, b as u64);
        }
    }
    if from.count > 0 {
        w.write(from.count, from.accumulator & ((1 << from.count) - 1));
    }
}

fn write_utf8(w: &mut BitWriter, value: u64) {
    if value < 0x80 {
        w.write(8, value);
        return;
    }
    let mut extra = 1;
    while extra < 6 && value >= 1u64 << (6 + 5 * extra) {
        extra += 1;
    }
    let lead_bits = 6 - extra; // payload bits in the first byte
    let lead = (0xffu64 << (7 - extra)) & 0xff;
    w.write(8, lead | (value >> (6 * extra)) & ((1 << lead_bits) - 1));
    for i in (0..extra).rev() {
        w.write(8, 0x80 | ((value >> (6 * i)) & 0x3f));
    }
}

/// The smallest subframe for `x` (samples of `bits` bits).
fn encode_subframe(x: &[i64], bits: u32) -> BitWriter {
    let n = x.len();
    // Constant.
    if x.iter().all(|&s| s == x[0]) {
        let mut w = BitWriter::new();
        w.write(8, 0);
        w.write_signed(bits, x[0]);
        return w;
    }
    // Wasted bits: low bits zero in every sample.
    let wasted = x.iter().fold(0i64, |acc, &s| acc | s).trailing_zeros().min(bits - 1);
    let shifted: Vec<i64>;
    let (x, bits) = if wasted > 0 {
        shifted = x.iter().map(|&s| s >> wasted).collect();
        (&shifted[..], bits - wasted)
    } else {
        (x, bits)
    };
    let header = |w: &mut BitWriter, kind: u64| {
        w.write(1, 0);
        w.write(6, kind);
        if wasted > 0 {
            w.write(1, 1);
            w.write_unary(wasted - 1);
        } else {
            w.write(1, 0);
        }
    };

    // Pick the cheapest predictor by estimated size, then write only it.
    enum Predictor {
        Verbatim,
        Fixed(usize),
        Lpc(Vec<i32>, u32, u32),
    }
    let mut best = (bits as u64 * n as u64, Predictor::Verbatim, None::<Plan>);
    let mut residual = vec![0i64; n];
    let mut best_residual = vec![0i64; n];
    for order in 0..=4.min(n) {
        fixed_residual(x, order, &mut residual);
        if let Some(plan) = plan_residual(&residual[order..], order, n) {
            let size = (order as u32 * bits) as u64 + plan.bits;
            if size < best.0 {
                best = (size, Predictor::Fixed(order), Some(plan));
                best_residual.copy_from_slice(&residual);
            }
        }
    }
    if n > MAX_LPC_ORDER * 2 {
        for (order, coefficients, shift, precision) in lpc_candidates(x, bits) {
            if lpc_residual(x, &coefficients, shift, &mut residual).is_none() {
                continue;
            }
            if let Some(plan) = plan_residual(&residual[order..], order, n) {
                let size = (order as u32 * (bits + precision) + 9) as u64 + plan.bits;
                if size < best.0 {
                    best = (size, Predictor::Lpc(coefficients, shift, precision), Some(plan));
                    best_residual.copy_from_slice(&residual);
                }
            }
        }
    }

    let mut w = BitWriter::new();
    match (best.1, best.2) {
        (Predictor::Fixed(order), Some(plan)) => {
            header(&mut w, 8 + order as u64);
            for &s in &x[..order] {
                w.write_signed(bits, s);
            }
            write_residual(&mut w, &best_residual[order..], order, n, &plan);
        }
        (Predictor::Lpc(coefficients, shift, precision), Some(plan)) => {
            let order = coefficients.len();
            header(&mut w, 31 + order as u64);
            for &s in &x[..order] {
                w.write_signed(bits, s);
            }
            w.write(4, precision as u64 - 1);
            w.write_signed(5, shift as i64);
            for &c in &coefficients {
                w.write_signed(precision, c as i64);
            }
            write_residual(&mut w, &best_residual[order..], order, n, &plan);
        }
        _ => {
            header(&mut w, 1);
            for &s in x {
                w.write_signed(bits, s);
            }
        }
    }
    w
}

fn fixed_residual(x: &[i64], order: usize, out: &mut [i64]) {
    for i in order..x.len() {
        out[i] = match order {
            0 => x[i],
            1 => x[i] - x[i - 1],
            2 => x[i] - 2 * x[i - 1] + x[i - 2],
            3 => x[i] - 3 * x[i - 1] + 3 * x[i - 2] - x[i - 3],
            _ => x[i] - 4 * x[i - 1] + 6 * x[i - 2] - 4 * x[i - 3] + x[i - 4],
        };
    }
}

/// The residual of quantized LPC; None if it doesn't fit 32 bits.
fn lpc_residual(x: &[i64], coefficients: &[i32], shift: u32, out: &mut [i64]) -> Option<()> {
    let order = coefficients.len();
    for i in order..x.len() {
        let mut sum = 0i64;
        for (j, &c) in coefficients.iter().enumerate() {
            sum += c as i64 * x[i - 1 - j];
        }
        let r = x[i] - (sum >> shift);
        if r.unsigned_abs() >= 1 << 30 {
            return None;
        }
        out[i] = r;
    }
    Some(())
}

/// Quantized LPC predictors to try: (order, coefficients, shift, precision).
fn lpc_candidates(x: &[i64], bits: u32) -> Vec<(usize, Vec<i32>, u32, u32)> {
    let n = x.len();
    // Autocorrelation of the windowed (Tukey 0.5) block.
    let taper = n / 4;
    let windowed: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = if i < taper {
                0.5 - 0.5 * (std::f64::consts::PI * i as f64 / taper as f64).cos()
            } else if i >= n - taper {
                0.5 - 0.5 * (std::f64::consts::PI * (n - 1 - i) as f64 / taper as f64).cos()
            } else {
                1.0
            };
            s as f64 * w
        })
        .collect();
    let mut autocorrelation = [0f64; MAX_LPC_ORDER + 1];
    for (lag, a) in autocorrelation.iter_mut().enumerate() {
        *a = windowed[lag..].iter().zip(&windowed).map(|(a, b)| a * b).sum();
    }
    if autocorrelation[0] <= 0.0 {
        return Vec::new();
    }
    autocorrelation[0] *= 1.0 + 1e-9;

    // Levinson-Durbin: the predictor of every order, and its error.
    let mut lpc = [[0f64; MAX_LPC_ORDER]; MAX_LPC_ORDER];
    let mut errors = [0f64; MAX_LPC_ORDER];
    let mut current = [0f64; MAX_LPC_ORDER];
    let mut error = autocorrelation[0];
    for i in 0..MAX_LPC_ORDER {
        let mut r = -autocorrelation[i + 1];
        for j in 0..i {
            r -= current[j] * autocorrelation[i - j];
        }
        r /= error;
        let previous = current;
        current[i] = r;
        for j in 0..i {
            current[j] = previous[j] + r * previous[i - 1 - j];
        }
        error *= 1.0 - r * r;
        for j in 0..=i {
            lpc[i][j] = -current[j];
        }
        errors[i] = error.max(1e-12);
    }

    // The orders whose estimated size is smallest, best first.
    let precision = 15;
    let mut estimates: Vec<(f64, usize)> = (1..=MAX_LPC_ORDER)
        .map(|order| {
            let per_sample = (0.5 * (errors[order - 1] / n as f64).log2()).max(0.0) + 1.0;
            (per_sample * (n - order) as f64 + (order as u32 * (precision + bits)) as f64, order)
        })
        .collect();
    estimates.sort_by(|a, b| a.0.total_cmp(&b.0));
    estimates
        .into_iter()
        .take(3)
        .filter_map(|(_, order)| {
            let (coefficients, shift) = quantize(&lpc[order - 1][..order], precision)?;
            Some((order, coefficients, shift, precision))
        })
        .collect()
}

/// Coefficients as `precision`-bit integers and the shift that scales them.
fn quantize(lpc: &[f64], precision: u32) -> Option<(Vec<i32>, u32)> {
    let max = lpc.iter().fold(0f64, |m, c| m.max(c.abs()));
    if max <= 0.0 || !max.is_finite() {
        return None;
    }
    let limit = (1i32 << (precision - 1)) - 1;
    // log2 of the largest coefficient, rounded up.
    let log2 = max.log2().floor() as i32 + 1;
    let shift = (precision as i32 - 1 - log2).clamp(0, 15) as u32;
    let mut error = 0.0;
    let mut out = Vec::with_capacity(lpc.len());
    for &c in lpc {
        error += c * (1u32 << shift) as f64;
        let q = error.round().clamp(-(limit as f64) - 1.0, limit as f64);
        out.push(q as i32);
        error -= q;
    }
    Some((out, shift))
}

/// How a residual is Rice coded: its partition order, each partition's
/// parameter, and the estimated size in bits.
struct Plan {
    bits: u64,
    partition_order: u32,
    parameters: Vec<u32>,
}

#[inline(always)]
fn fold(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// The partitioning and parameters that code `residual` (the block's
/// samples after the `order` warm-up ones) in about the fewest bits. None
/// if a sample is too large for any parameter.
fn plan_residual(residual: &[i64], order: usize, n: usize) -> Option<Plan> {
    // The deepest partition order the block allows.
    let mut max_order = 0;
    while max_order < MAX_PARTITION_ORDER && n % (2 << max_order) == 0 && n >> (max_order + 1) > order {
        max_order += 1;
    }
    // Sums of the folded values in the finest partitions, merged pairwise
    // for each coarser order.
    let size = n >> max_order;
    let mut sums = Vec::with_capacity(1 << max_order);
    let mut counts = Vec::with_capacity(1 << max_order);
    let mut at = 0;
    for p in 0..1usize << max_order {
        let count = size - if p == 0 { order } else { 0 };
        sums.push(residual[at..at + count].iter().map(|&r| fold(r)).sum::<u64>());
        counts.push(count as u64);
        at += count;
    }
    let mut best: Option<Plan> = None;
    for partition_order in (0..=max_order).rev() {
        let mut bits = 6;
        let mut parameters = Vec::with_capacity(sums.len());
        for (&sum, &count) in sums.iter().zip(&counts) {
            let (parameter, cost) = rice_parameter(sum, count);
            parameters.push(parameter);
            bits += cost + 4;
        }
        if parameters.iter().any(|&p| p > 14) {
            bits += parameters.len() as u64;
        }
        if best.as_ref().is_none_or(|b| bits < b.bits) {
            best = Some(Plan { bits, partition_order, parameters });
        }
        if partition_order > 0 {
            sums = sums.chunks(2).map(|c| c[0] + c[1]).collect();
            counts = counts.chunks(2).map(|c| c[0] + c[1]).collect();
        }
    }
    best.filter(|plan| plan.parameters.iter().all(|&p| p <= 30))
}

/// The Rice parameter for a partition of `count` folded values summing to
/// `sum`, and the partition's size with it (estimated from the sum).
fn rice_parameter(sum: u64, count: u64) -> (u32, u64) {
    if count == 0 {
        return (0, 0);
    }
    let mean = sum / count;
    let guess = if mean == 0 { 0 } else { 64 - mean.leading_zeros() };
    let cost = |k: u32| count * (k as u64 + 1) + (sum >> k);
    let mut best = (guess, cost(guess));
    for k in [guess.saturating_sub(1), guess + 1] {
        if cost(k) < best.1 {
            best = (k, cost(k));
        }
    }
    best
}

fn write_residual(w: &mut BitWriter, residual: &[i64], order: usize, n: usize, plan: &Plan) {
    let wide = plan.parameters.iter().any(|&p| p > 14);
    w.write(2, wide as u64);
    w.write(4, plan.partition_order as u64);
    let partitions = 1usize << plan.partition_order;
    let mut at = 0;
    for (p, &parameter) in plan.parameters.iter().enumerate() {
        let count = n / partitions - if p == 0 { order } else { 0 };
        w.write(if wide { 5 } else { 4 }, parameter as u64);
        let mask = (1u64 << parameter) - 1;
        for &r in &residual[at..at + count] {
            let u = fold(r);
            w.write_unary((u >> parameter) as u32);
            w.write(parameter, u & mask);
        }
        at += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn tone(frames: usize, bits: u32, seed: u32) -> Vec<i32> {
        let mut seed = seed;
        let full = (1i64 << (bits - 1)) as f32 - 1.0;
        (0..frames)
            .map(|i| {
                let t = i as f32 / 48000.0;
                let s = 0.5 * (t * 440.0 * std::f32::consts::TAU).sin() * (-t).exp()
                    + 0.2 * (t * 1320.7 * std::f32::consts::TAU).sin()
                    + 0.01 * noise(&mut seed);
                (s * full) as i32
            })
            .collect()
    }

    #[test]
    fn reads_a_unary_code_ending_the_cache() {
        // 63 zeros and a 1 fill the 64-bit cache exactly.
        let data = [0, 0, 0, 0, 0, 0, 0, 1, 0xab, 0xcd, 0xef, 0x12];
        let mut r = BitReader::new(&data, 0);
        assert_eq!(r.read_unary().unwrap(), 63);
        assert_eq!(r.read(8).unwrap(), 0xab);
        assert_eq!(r.read(16).unwrap(), 0xcdef);
    }

    #[test]
    fn md5_matches_the_reference() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let digest = |s: &[u8]| {
            let mut m = Md5::new();
            m.update(s);
            hex(m.finish())
        };
        assert_eq!(digest(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(digest(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(digest(&[b'a'; 1000]), "cabe45dcc9ae5b66ba86600cca6b8ba8");
    }

    #[test]
    fn round_trips_every_shape() {
        for (channels, bits, frames) in
            [(1, 16, 10000), (2, 16, 9000), (2, 24, 5000), (1, 8, 300), (3, 12, 4096), (2, 16, 1)]
        {
            let data: Vec<Vec<i32>> = (0..channels).map(|c| tone(frames, bits, 7 + c as u32)).collect();
            let slices: Vec<&[i32]> = data.iter().map(|c| &c[..]).collect();
            let encoded = encode(&slices, 48000, bits);
            let decoded = decode(&encoded).unwrap();
            assert_eq!(decoded.sample_rate, 48000);
            assert_eq!(decoded.bits_per_sample, bits);
            assert_eq!(decoded.channels, data, "{channels} ch, {bits} bit, {frames} frames");
        }
    }

    #[test]
    fn round_trips_silence_extremes_and_wasted_bits() {
        let frames = 5000;
        let silence = vec![0i32; frames];
        let extremes: Vec<i32> = (0..frames).map(|i| if i % 2 == 0 { 32767 } else { -32768 }).collect();
        let wasted: Vec<i32> = tone(frames, 16, 3).iter().map(|s| s & !0xff).collect();
        for data in [&silence, &extremes, &wasted] {
            let encoded = encode(&[data, data], 44100, 16);
            assert_eq!(decode(&encoded).unwrap().channels, vec![data.clone(), data.clone()]);
        }
    }

    #[test]
    fn compresses_a_tone() {
        let data = tone(48000, 16, 1);
        let encoded = encode(&[&data], 48000, 16);
        assert!(encoded.len() < data.len() * 2 * 3 / 4, "{} bytes for {} samples", encoded.len(), data.len());
    }

    /// The signal the files in testdata/flac were made from (by the
    /// reference encoder, `flac`): interleaved, integer-only.
    fn fixture_signal(frames: usize, channels: usize, bits: u32) -> Vec<Vec<i32>> {
        let full = 1i64 << (bits - 1);
        let mut x = 7i64;
        let mut out = vec![Vec::with_capacity(frames); channels];
        for i in 0..frames as i64 {
            for (c, channel) in out.iter_mut().enumerate() {
                x = (x * 1103515245 + 12345) & 0x7fff_ffff;
                let triangle = (i * (37 + 11 * c as i64)) % 2000 - 1000;
                let v = triangle * (full / 1100) + (x >> 16) % 64 - 32;
                channel.push(v.clamp(-full, full - 1) as i32);
            }
        }
        out
    }

    #[test]
    fn decodes_the_reference_encoder() {
        for (file, frames, channels, bits, rate) in [
            ("stereo16.flac", 12000, 2, 16, 48000),
            ("mono24.flac", 7000, 1, 24, 96000),
            ("odd_blocks.flac", 5000, 2, 16, 44100),
            ("mono8.flac", 3000, 1, 8, 22050),
        ] {
            let path = format!("{}/testdata/flac/{file}", env!("CARGO_MANIFEST_DIR"));
            let audio = decode(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!((audio.sample_rate, audio.bits_per_sample), (rate, bits), "{file}");
            assert!(audio.channels == fixture_signal(frames, channels, bits), "{file}");
        }
    }

    #[test]
    fn signs_the_audio_as_the_reference_encoder_does() {
        // STREAMINFO's MD5 in stereo16.flac, written by `flac`.
        let path = format!("{}/testdata/flac/stereo16.flac", env!("CARGO_MANIFEST_DIR"));
        let file = std::fs::read(path).unwrap();
        let signal = fixture_signal(12000, 2, 16);
        let ours = encode(&[&signal[0], &signal[1]], 48000, 16);
        assert_eq!(&ours[8 + 18..8 + 34], &file[8 + 18..8 + 34]);
    }

    #[test]
    fn survives_random_damage() {
        let path = format!("{}/testdata/flac/stereo16.flac", env!("CARGO_MANIFEST_DIR"));
        let original = std::fs::read(path).unwrap();
        let mut seed = 0x1234_5678u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize
        };
        for _ in 0..3000 {
            let mut bytes = original.clone();
            for _ in 0..1 + random() % 8 {
                let at = random() % bytes.len();
                bytes[at] = random() as u8;
            }
            if random() % 5 == 0 {
                bytes.truncate(random() % bytes.len());
            }
            let _ = decode(&bytes);
            let _ = stream_info(&bytes);
        }
    }

    #[test]
    fn rejects_damage() {
        let data = tone(9000, 16, 5);
        let mut encoded = encode(&[&data], 48000, 16);
        assert!(decode(b"RIFF").is_err());
        let middle = encoded.len() / 2;
        encoded[middle] ^= 0x55;
        // A damaged frame fails its CRC and is missing from the output.
        assert!(decode(&encoded).is_err());
    }
}
