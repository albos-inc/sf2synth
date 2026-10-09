//! The C interface, called as C would call it.

use std::ffi::{CStr, CString};
use std::ptr::{null, null_mut};

use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};
use sf2synth_c::*;

fn font_bytes() -> Vec<u8> {
    let points = (0..44100).map(|i| ((i as f64 / 100.0 * std::f64::consts::TAU).sin() * 16000.0) as i16).collect();
    let mut builder = Sf2Builder::new("Test");
    let sample = builder.add_sample(SampleDef::mono("Sine", points, 44100, 69).looped(1000, 41000));
    let instrument = builder.add_instrument(InstrumentDef::new("Sine").zone(ZoneDef::sample(sample).generator(54, 1)));
    builder.add_preset(PresetDef::new("Sine", 0, 0).zone(ZoneDef::instrument(instrument)));
    builder.add_preset(PresetDef::new("Kit", 128, 0).zone(ZoneDef::instrument(instrument)));
    builder.to_bytes()
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, &x| m.max(x.abs()))
}

#[test]
fn loads_and_plays() {
    let path = std::env::temp_dir().join(format!("sf2synth-capi-{}.sf2", std::process::id()));
    std::fs::write(&path, font_bytes()).unwrap();
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    unsafe {
        for font in
            [sf2synth_font_load(c_path.as_ptr(), null_mut()), sf2synth_font_load_mapped(c_path.as_ptr(), null_mut())]
        {
            assert!(!font.is_null());
            assert_eq!(sf2synth_font_preset_count(font), 2);
            let (mut bank, mut program) = (0u16, 0u16);
            let mut name = [0 as std::ffi::c_char; 3];
            assert!(sf2synth_font_preset(font, 1, &mut bank, &mut program, name.as_mut_ptr(), name.len()));
            assert_eq!((bank, program), (128, 0));
            assert_eq!(CStr::from_ptr(name.as_ptr()).to_str().unwrap(), "Ki");
            assert!(!sf2synth_font_preset(font, 2, null_mut(), null_mut(), null_mut(), 0));

            let mut settings = sf2synth_settings_default(44100);
            assert_eq!(settings.master_gain, 1.0);
            assert_eq!(settings.drum_channel, 9);
            settings.reverb = false;
            let synth = sf2synth_new(font, &settings, null_mut());
            assert!(!synth.is_null());
            // The synth keeps the font alive.
            sf2synth_font_free(font);

            let frames = 1024;
            let (mut left, mut right, mut reverb) = (vec![1.0f32; frames], vec![1.0f32; frames], vec![1.0f32; frames]);
            let cc91 = [0xb0u8, 91, 127];
            let note_on = [0x90u8, 69, 100];
            let gs_reset = [0xf0u8, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7];
            let events = [
                MidiEvent { frame: 0, length: 11, bytes: gs_reset.as_ptr() },
                MidiEvent { frame: 0, length: 3, bytes: cc91.as_ptr() },
                MidiEvent { frame: 512, length: 3, bytes: note_on.as_ptr() },
            ];
            sf2synth_render_events(
                synth,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                reverb.as_mut_ptr(),
                null_mut(),
                frames,
                events.as_ptr(),
                events.len(),
            );
            assert_eq!(peak(&left[..512]), 0.0);
            assert!(peak(&left[600..]) > 0.01);
            assert!(peak(&reverb[600..]) > 0.001);
            assert_eq!(sf2synth_active_voices(synth), 1);

            sf2synth_midi_bytes(synth, [0x80, 69].as_ptr(), 2);
            sf2synth_midi_bytes(synth, [0].as_ptr(), 1);
            sf2synth_render(synth, left.as_mut_ptr(), right.as_mut_ptr(), frames);
            sf2synth_ump(synth, [0x2090_4564u32].as_ptr(), 1);
            sf2synth_set_channel_drum(synth, 3, true);
            sf2synth_set_reverb_enabled(synth, true);
            assert!(sf2synth_set_sample_rate(synth, 48000));
            assert_eq!(sf2synth_sample_rate(synth), 48000);
            assert!(!sf2synth_set_sample_rate(synth, 1));
            sf2synth_reset(synth);
            sf2synth_free(synth);
        }

        // Errors.
        let mut error = null_mut();
        let missing = CString::new("/nonexistent.sf2").unwrap();
        assert!(sf2synth_font_load_mapped(missing.as_ptr(), &mut error).is_null());
        assert!(!error.is_null());
        sf2synth_string_free(error);
        assert!(sf2synth_font_load_memory(b"nope".as_ptr(), 4, null_mut()).is_null());
        assert!(sf2synth_new(null(), null(), null_mut()).is_null());
        // NULL synths are ignored.
        sf2synth_midi(null_mut(), [0x90u8, 60, 100].as_ptr(), 3);
        sf2synth_render(null_mut(), null_mut(), null_mut(), 0);
    }
    std::fs::remove_file(&path).unwrap();
}
