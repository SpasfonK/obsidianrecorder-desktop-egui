//! Traitement du signal audio « pur » : sous-échantillonnage vers mono et
//! ré-échantillonnage linéaire vers 16 kHz.
//!
//! Aucune de ces fonctions ne dépend d'une API système ni d'un périphérique
//! réel : elles peuvent donc être testées unitairement avec un flux audio
//! simulé, exactement comme le pipeline de production (voir
//! `super::capture`) les enchaîne sur les échantillons reçus du callback
//! `cpal`.

/// Fréquence d'échantillonnage cible, imposée par les moteurs STT hors ligne
/// visés en Itération 2 (Vosk et whisper.cpp attendent tous deux du PCM
/// mono 16 kHz).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Ramène un buffer entrelacé multi-canaux (flottant, plage [-1.0, 1.0]) à
/// un unique canal mono, en moyennant les canaux de chaque trame.
pub fn downmix_f32_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Variante pour un buffer entrelacé PCM 16 bits signé (certains
/// périphériques n'exposent que ce format en sortie de `cpal`).
pub fn downmix_i16_to_mono_f32(interleaved: &[i16], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    interleaved
        .chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum();
            sum / frame.len() as f32
        })
        .collect()
}

/// Ré-échantillonnage linéaire (interpolation) d'un signal mono vers une
/// fréquence cible.
///
/// Suffisant pour de la voix en 16 kHz ; un ré-échantillonneur plus
/// sophistiqué (sinc windowé) pourra remplacer cette implémentation plus
/// tard sans changer l'interface publique du module `capture`.
pub fn resample_linear(mono: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if mono.is_empty() || from_rate == to_rate || from_rate == 0 {
        return mono.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((mono.len() as f64) * ratio).round() as usize;
    if out_len == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(out_len);
    let last_index = mono.len() - 1;
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = (src_pos.floor() as usize).min(last_index);
        let frac = (src_pos - idx as f64) as f32;
        let next = (idx + 1).min(last_index);
        out.push(mono[idx] * (1.0 - frac) + mono[next] * frac);
    }
    out
}

/// Quantifie un signal flottant [-1.0, 1.0] vers du PCM 16 bits signé, avec
/// écrêtage de sécurité pour éviter tout dépassement d'entier sur un signal
/// mal calibré.
pub fn quantize_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * i16::MAX as f32).round() as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_f32_stereo_averages_channels() {
        // Gauche = 1.0, Droite = -1.0 → moyenne = 0.0 pour la première trame.
        let stereo = [1.0_f32, -1.0, 0.5, 0.5];
        let mono = downmix_f32_to_mono(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let mono_in = [0.1_f32, 0.2, -0.3];
        assert_eq!(downmix_f32_to_mono(&mono_in, 1), mono_in.to_vec());
    }

    #[test]
    fn downmix_i16_stereo_averages_channels() {
        let stereo = [i16::MAX, i16::MIN + 1, 0, 0];
        let mono = downmix_i16_to_mono_f32(&stereo, 2);
        assert!((mono[0]).abs() < 1e-3, "canaux opposés → ~0.0, obtenu {}", mono[0]);
        assert_eq!(mono[1], 0.0);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let signal = [0.1_f32, 0.2, 0.3, 0.4];
        assert_eq!(resample_linear(&signal, 16_000, 16_000), signal.to_vec());
    }

    #[test]
    fn resample_downsamples_44100_to_16000_with_expected_length() {
        let input_len = 44_100usize; // 1 seconde à 44,1 kHz
        let signal: Vec<f32> = (0..input_len)
            .map(|i| (i as f32 / input_len as f32) * 2.0 - 1.0)
            .collect();
        let resampled = resample_linear(&signal, 44_100, TARGET_SAMPLE_RATE);
        // ~1 seconde à 16 kHz : on tolère une marge de quelques échantillons.
        assert!((resampled.len() as i64 - TARGET_SAMPLE_RATE as i64).abs() <= 2);
    }

    #[test]
    fn resample_empty_signal_stays_empty() {
        let empty: Vec<f32> = Vec::new();
        assert!(resample_linear(&empty, 44_100, TARGET_SAMPLE_RATE).is_empty());
    }

    #[test]
    fn quantize_clips_out_of_range_values() {
        let samples = [2.0_f32, -2.0, 0.0];
        let pcm = quantize_to_i16(&samples);
        assert_eq!(pcm, vec![i16::MAX, -i16::MAX, 0]);
    }

    #[test]
    fn quantize_preserves_ordering_of_moderate_values() {
        let samples = [-0.5_f32, 0.0, 0.5];
        let pcm = quantize_to_i16(&samples);
        assert!(pcm[0] < pcm[1] && pcm[1] < pcm[2]);
    }
}
