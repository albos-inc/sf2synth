//! A minimal 32-bit float WAV writer shared by the examples.

use std::io::{self, Write};

/// Writes interleaved stereo `samples` as a 32-bit float WAV file.
pub fn write_wav(path: &str, sample_rate: u32, left: &[f32], right: &[f32]) -> io::Result<()> {
    let frames = left.len().min(right.len());
    let data_bytes = (frames * 2 * 4) as u32;
    let mut out = io::BufWriter::new(std::fs::File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&3u16.to_le_bytes())?; // IEEE float
    out.write_all(&2u16.to_le_bytes())?;
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&(sample_rate * 8).to_le_bytes())?;
    out.write_all(&8u16.to_le_bytes())?;
    out.write_all(&32u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;
    for i in 0..frames {
        out.write_all(&left[i].to_le_bytes())?;
        out.write_all(&right[i].to_le_bytes())?;
    }
    out.flush()
}
