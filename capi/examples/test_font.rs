//! Writes a small SoundFont for tests (the Swift package's in CI): a looping
//! sine as a melodic preset (bank 0, program 0) and a drum kit (bank 128).
//!
//! ```text
//! cargo run --manifest-path capi/Cargo.toml --example test_font -- test.sf2
//! ```

use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "test.sf2".to_string());
    // 441 Hz at 44.1 kHz: 100 points per period, played as key 69.
    let points = (0..44_100).map(|i| ((i as f64 / 100.0 * std::f64::consts::TAU).sin() * 16_000.0) as i16).collect();
    let mut builder = Sf2Builder::new("sf2synth test");
    let sample = builder.add_sample(SampleDef::mono("Sine", points, 44_100, 69).looped(1_000, 41_000));
    // sampleModes 1: loop.
    let instrument = builder.add_instrument(InstrumentDef::new("Sine").zone(ZoneDef::sample(sample).generator(54, 1)));
    builder.add_preset(PresetDef::new("Sine", 0, 0).zone(ZoneDef::instrument(instrument)));
    builder.add_preset(PresetDef::new("Kit", 128, 0).zone(ZoneDef::instrument(instrument)));
    std::fs::write(&path, builder.to_bytes())?;
    println!("wrote {path}");
    Ok(())
}
