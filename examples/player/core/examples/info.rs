//! Prints what the player apps show about an SF2 and a MIDI file, and plays
//! the song silently (faster than real time) through the transport.
//!
//! ```text
//! cargo run --release -p sf2player --example info -- <font.sf2> <song.mid>
//! ```

use sf2player::{LoadedSong, LoadedSoundFont, Player, State, format_time};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, font_path, song_path] = &args[..] else {
        eprintln!("usage: info <font.sf2> <song.mid>");
        std::process::exit(2);
    };
    let sample_rate = 48_000;
    let font = LoadedSoundFont::new(std::fs::read(font_path)?, font_path, sample_rate)?;
    let song = LoadedSong::new(&std::fs::read(song_path)?, song_path)?;
    println!("{:#?}\n{:#?}", font.summary(), song.summary());

    let mut player = Player::new(sample_rate);
    player.set_sound_font(font);
    player.set_song(song);
    player.play();
    let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
    let (mut peak, mut peak_voices, mut frames) = (0.0f32, 0, 0u64);
    while player.state() == State::Playing {
        player.render(&mut left, &mut right);
        peak = left.iter().chain(&right).fold(peak, |m, &x| m.max(x.abs()));
        peak_voices = peak_voices.max(player.active_voices());
        frames += 512;
    }
    println!(
        "played {} of {}: peak {peak:.3}, up to {peak_voices} voices",
        format_time(frames as f64 / sample_rate as f64),
        format_time(player.duration())
    );
    Ok(())
}
