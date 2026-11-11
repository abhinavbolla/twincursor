//! WAV files for the test tools, in the one format the voice path uses: 48 kHz, mono, 16-bit.

use std::path::Path;

use super::marker::RATE;

#[derive(Debug, thiserror::Error)]
pub enum WavError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not a WAV file")]
    NotWav,
    #[error("no sound in the file")]
    Empty,
    #[error(
        "{channels} channel(s) at {rate} Hz, {bits}-bit: the file must be mono, 48000 Hz, 16-bit \
         PCM (`afconvert -f WAVE -d LEI16@48000 -c 1 in.wav out.wav` converts one)"
    )]
    Unsupported { channels: u16, rate: u32, bits: u16 },
}

/// The samples as a WAV file's bytes.
pub fn encode(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    // PCM, mono, 48 kHz, 2 bytes a frame, 16 bits.
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

/// The samples in a WAV file's bytes. Chunks other than `fmt ` and `data` are skipped (macOS
/// tools pad the header with one).
pub fn decode(bytes: &[u8]) -> Result<Vec<i16>, WavError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut rest = &bytes[12..];
    let mut format_ok = false;
    while rest.len() >= 8 {
        let size = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
        // A file that was still being written can claim more than it holds.
        let body = &rest[8..rest.len().min(8 + size)];
        match &rest[..4] {
            b"fmt " if body.len() >= 16 => {
                let field = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
                // 1 is PCM; 0xFFFE says "look further in", where the same number sits.
                let pcm = match field(0) {
                    1 => true,
                    0xFFFE => body.len() >= 26 && field(24) == 1,
                    _ => false,
                };
                let (channels, bits) = (field(2), field(14));
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                if !pcm || channels != 1 || rate != RATE || bits != 16 {
                    return Err(WavError::Unsupported {
                        channels,
                        rate,
                        bits,
                    });
                }
                format_ok = true;
            }
            b"data" if format_ok => {
                let samples: Vec<i16> = body
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&b| i16::from_le_bytes(b))
                    .collect();
                return if samples.is_empty() {
                    Err(WavError::Empty)
                } else {
                    Ok(samples)
                };
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        rest = &rest[rest.len().min(8 + size + size % 2)..];
    }
    Err(WavError::NotWav)
}

pub fn read(path: &Path) -> Result<Vec<i16>, WavError> {
    decode(&std::fs::read(path)?)
}

pub fn write(path: &Path, samples: &[i16]) -> Result<(), WavError> {
    Ok(std::fs::write(path, encode(samples))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let samples = [0, 1, -1, i16::MAX, i16::MIN, 1234];
        assert_eq!(decode(&encode(&samples)).unwrap(), samples);
    }

    #[test]
    fn skips_chunks_it_doesnt_know() {
        // As `say` and `afconvert` write them: a filler chunk between the format and the data.
        let plain = encode(&[5, -6, 7]);
        let mut padded = plain[..36].to_vec();
        padded.extend_from_slice(b"FLLR");
        padded.extend_from_slice(&3u32.to_le_bytes());
        padded.extend_from_slice(&[0; 4]);
        padded.extend_from_slice(&plain[36..]);
        assert_eq!(decode(&padded).unwrap(), [5, -6, 7]);
    }

    #[test]
    fn says_what_is_wrong_with_other_formats() {
        let mut stereo = encode(&[1, 2, 3, 4]);
        stereo[22] = 2;
        assert!(matches!(
            decode(&stereo),
            Err(WavError::Unsupported { channels: 2, .. })
        ));
        let mut slow = encode(&[1, 2]);
        slow[24..28].copy_from_slice(&44_100u32.to_le_bytes());
        let message = decode(&slow).unwrap_err().to_string();
        assert!(message.contains("44100 Hz"), "{message}");
        assert!(message.contains("afconvert"), "{message}");
    }

    #[test]
    fn rejects_what_isnt_a_wav_or_has_no_sound() {
        assert!(matches!(decode(b"not a wav at all"), Err(WavError::NotWav)));
        assert!(matches!(decode(&encode(&[])), Err(WavError::Empty)));
        // Cut off inside the header.
        assert!(matches!(decode(&encode(&[1])[..30]), Err(WavError::NotWav)));
    }

    #[test]
    fn reads_a_file_cut_short() {
        let whole = encode(&[10, 20, 30, 40]);
        assert_eq!(decode(&whole[..whole.len() - 3]).unwrap(), [10, 20]);
    }
}
