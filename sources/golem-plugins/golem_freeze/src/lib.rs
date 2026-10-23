//! GoleM Freeze — le gel spectral (docs/spectral.md §3).
//!
//! Le geste de la pédale EHX Freeze : un accord, le pied, et il TIENT — on joue par-dessus. Ici en
//! spectral (pas en granulaire : le spectral n'a pas le grain, il a la note) :
//!
//! - le direct passe TOUJOURS : chaque trame d'entrée traverse l'analyse/synthèse STFT à l'identique
//!   (Hann × Hann, recouvrement 4, gain compensé) — c'est ce qui porte le direct avec exactement la
//!   latence déclarée, sans mixeur sec/mouillé à part ;
//! - à l'engagement (`Freeze` passe à ON), les AMPLITUDES des `CAPTURE_FRAMES` premières trames sont
//!   moyennées (ne pas geler un transitoire) et les phases de la première trame sont gardées, pour que
//!   la première trame de synthèse soit celle qu'on vient d'entendre ;
//! - à la capture on mesure aussi la FRÉQUENCE INSTANTANÉE de chaque bin (avance de phase entre deux
//!   trames, ±π autour du centre du bin) : ensuite chaque trame de sortie = amplitudes gelées × phase
//!   qui tourne À CETTE VITESSE — c'est le phase vocoder classique ; faire tourner au centre du bin
//!   aimante les partiels sur une grille de 23,4 Hz et le gel « se répète » à la cadence des trames
//!   (v0.1, jugée à l'oreille le 2026-09-25) ; + une gigue de phase douce (`Smear`) qui décorrèle les
//!   bins ; `Tone` incline le spectre gelé (± 6 dB/oct autour de 1 kHz) ;
//! - au relâchement, le gel s'éteint en `Decay` secondes (exponentielle par trame).
//!
//! `Freeze` est un PARAMÈTRE, pas le bypass : le rack l'actionne comme l'« Enable » du convolveur
//! x42 (`golem_rig.fx_set_on`), donc la latence ne bouge jamais et la queue de relâchement sonne.

use nih_plug::prelude::*;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

const MIN_WINDOW_ORDER: usize = 10; // 1024
const MAX_WINDOW_ORDER: usize = 12; // 4096
const MAX_WINDOW_SIZE: usize = 1 << MAX_WINDOW_ORDER;
const OVERLAP_TIMES: usize = 4;
const CAPTURE_FRAMES: u32 = 4;

#[derive(Enum, Debug, PartialEq, Clone, Copy)]
enum WindowSize {
    #[id = "1024"]
    #[name = "1024"]
    W1024,
    #[id = "2048"]
    #[name = "2048"]
    W2048,
    #[id = "4096"]
    #[name = "4096"]
    W4096,
}

impl WindowSize {
    fn order(self) -> usize {
        match self {
            WindowSize::W1024 => 10,
            WindowSize::W2048 => 11,
            WindowSize::W4096 => 12,
        }
    }
}

#[derive(Params)]
struct FreezeParams {
    /// Le geste : ON = capture puis tenue ; OFF = relâchement en `decay`.
    #[id = "freeze"]
    freeze: BoolParam,
    /// Niveau du gel ajouté au direct.
    #[id = "mix"]
    mix: FloatParam,
    /// Temps d'extinction au relâchement (secondes).
    #[id = "decay"]
    decay: FloatParam,
    /// Gigue de phase par trame : 0 = pur (métallique), 1 = nuage.
    #[id = "smear"]
    smear: FloatParam,
    /// Inclinaison du spectre gelé : 0 = sombre, 0,5 = neutre, 1 = brillant.
    #[id = "tone"]
    tone: FloatParam,
    /// Taille de fenêtre = latence (1024 / 2048 / 4096 échantillons).
    #[id = "window"]
    window: EnumParam<WindowSize>,
}

impl Default for FreezeParams {
    fn default() -> Self {
        Self {
            freeze: BoolParam::new("Freeze", false),
            mix: FloatParam::new("Mix", 0.8, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            decay: FloatParam::new(
                "Decay",
                0.6,
                FloatRange::Skewed { min: 0.05, max: 10.0, factor: FloatRange::skew_factor(-2.0) },
            )
            .with_unit(" s")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            smear: FloatParam::new("Smear", 0.2, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            tone: FloatParam::new("Tone", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            window: EnumParam::new("Window", WindowSize::W2048),
        }
    }
}

struct Plan {
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
}

pub struct GolemFreeze {
    params: Arc<FreezeParams>,
    stft: util::StftHelper,
    /// Un plan FFT par ordre de fenêtre (index = ordre − MIN_WINDOW_ORDER).
    plans: Vec<Plan>,
    /// Fenêtre de Hann de la taille courante.
    window_fn: Vec<f32>,
    complex: Vec<Complex32>,
    /// Par canal : amplitudes gelées, phase courante de synthèse, phase d'analyse de la trame
    /// précédente et VITESSE VRAIE de chaque bin (radians par saut), mesurée à la capture.
    frozen_mag: Vec<Vec<f32>>,
    phase: Vec<Vec<f32>>,
    prev_phase: Vec<Vec<f32>>,
    omega: Vec<Vec<f32>>,
    cur_order: usize,
    sample_rate: f32,
    /// Enveloppe du gel (1 tenu, → 0 au relâchement) ; `frozen` = l'état du geste vu par le DSP.
    env: f32,
    frozen: bool,
    /// Trames restant à moyenner après l'engagement (0 = capture faite).
    capture_left: u32,
    rng: u32,
}

impl Default for GolemFreeze {
    fn default() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let plans = (MIN_WINDOW_ORDER..=MAX_WINDOW_ORDER)
            .map(|o| Plan { r2c: planner.plan_fft_forward(1 << o), c2r: planner.plan_fft_inverse(1 << o) })
            .collect();
        Self {
            params: Arc::new(FreezeParams::default()),
            stft: util::StftHelper::new(2, MAX_WINDOW_SIZE, 0),
            plans,
            window_fn: util::window::hann(1 << 11),
            complex: vec![Complex32::default(); MAX_WINDOW_SIZE / 2 + 1],
            frozen_mag: vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; 2],
            phase: vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; 2],
            prev_phase: vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; 2],
            omega: vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; 2],
            cur_order: 11,
            sample_rate: 48_000.0,
            env: 0.0,
            frozen: false,
            capture_left: 0,
            rng: 0x9E37_79B9,
        }
    }
}

impl GolemFreeze {
    fn set_window(&mut self, order: usize) {
        let size = 1 << order;
        self.stft.set_block_size(size);
        self.window_fn = util::window::hann(size);
        self.cur_order = order;
        for ph in &mut self.phase {
            ph.iter_mut().for_each(|p| *p = 0.0);
        }
    }
}

impl Plugin for GolemFreeze {
    const NAME: &'static str = "GoleM Freeze";
    const VENDOR: &'static str = "GoleM";
    const URL: &'static str = "https://golem.fm";
    const EMAIL: &'static str = "contact@golem.fm";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let channels = audio_io_layout.main_output_channels.map(|c| c.get() as usize).unwrap_or(2);
        self.sample_rate = buffer_config.sample_rate;
        self.stft = util::StftHelper::new(channels, MAX_WINDOW_SIZE, 0);
        self.frozen_mag = vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; channels];
        self.phase = vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; channels];
        self.prev_phase = vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; channels];
        self.omega = vec![vec![0.0; MAX_WINDOW_SIZE / 2 + 1]; channels];
        let order = self.params.window.value().order();
        self.set_window(order);
        context.set_latency_samples(self.stft.latency_samples());
        true
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.frozen = false;
        self.capture_left = 0;
        for v in self.phase.iter_mut().chain(self.prev_phase.iter_mut()) {
            v.iter_mut().for_each(|p| *p = 0.0);
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // la fenêtre change : nouvelle taille de bloc, nouvelle latence (déclarée au hôte, PDC de Reaper)
        let order = self.params.window.value().order();
        if order != self.cur_order {
            self.set_window(order);
            context.set_latency_samples(self.stft.latency_samples());
        }
        let size = 1usize << order;
        let bins = size / 2 + 1;
        let hop = size / OVERLAP_TIMES;

        // le geste : front montant = capture + enveloppe pleine ; front descendant = relâchement
        let want = self.params.freeze.value();
        if want && !self.frozen {
            self.frozen = true;
            self.capture_left = CAPTURE_FRAMES;
            self.env = 1.0;
            for m in &mut self.frozen_mag {
                m.iter_mut().for_each(|x| *x = 0.0);
            }
        } else if !want && self.frozen {
            self.frozen = false;
        }
        let decay_k = (-(hop as f32) / (self.params.decay.value().max(0.01) * self.sample_rate)).exp();
        let mix = self.params.mix.value();
        let smear = self.params.smear.value();
        // tone : ± 6 dB/oct autour de 1 kHz, en gain linéaire par bin (pente en log2 de la fréquence)
        let tilt_db_per_oct = (self.params.tone.value() - 0.5) * 2.0 * 6.0;
        let bin_hz = self.sample_rate / size as f32;

        // compensation de gain (Hann² à recouvrement 4 somme à 1,5 ; la FFT réelle n'est pas normalisée)
        let gain_comp = ((OVERLAP_TIMES as f32 / 4.0) * 1.5).recip() / size as f32;
        let in_gain = gain_comp.sqrt();
        let out_gain = gain_comp.sqrt();

        let channels = self.frozen_mag.len();
        let plan = &self.plans[order - MIN_WINDOW_ORDER];
        let window_fn = &self.window_fn;
        let complex = &mut self.complex[..bins];
        let frozen_mag = &mut self.frozen_mag;
        let phase = &mut self.phase;
        let prev_phase = &mut self.prev_phase;
        let omega = &mut self.omega;
        let mut env = self.env;
        let frozen = self.frozen;
        let mut capture_left = self.capture_left;
        let mut rng = self.rng;
        let phase_step = 2.0 * std::f32::consts::PI / OVERLAP_TIMES as f32;

        self.stft.process_overlap_add(buffer, OVERLAP_TIMES, |channel_idx, real_fft_buffer| {
            for (s, w) in real_fft_buffer.iter_mut().zip(window_fn.iter()) {
                *s *= *w * in_gain;
            }
            plan.r2c.process(real_fft_buffer, complex).ok();

            let active = frozen || env > 1e-4;
            let prev = &mut prev_phase[channel_idx];
            if frozen && capture_left > 0 {
                // CAPTURE. Amplitudes : moyenne des premières trames (ne pas geler un transitoire).
                // Vitesse : la FRÉQUENCE INSTANTANÉE de chaque bin — l'avance de phase mesurée entre deux
                // trames, ramenée dans ±π autour de l'avance nominale du centre du bin. Sans elle, chaque
                // partiel est aimanté sur la grille des bins (23,4 Hz à 2048/48k) et le gel « se répète »
                // à la cadence des trames (mesuré à l'oreille, 2026-09-25) ; avec elle, la phase
                // tourne à la vraie hauteur du partiel : le gel est lisse.
                let mag = &mut frozen_mag[channel_idx];
                let ph = &mut phase[channel_idx];
                let om = &mut omega[channel_idx];
                let n_done = (CAPTURE_FRAMES - capture_left) as f32;
                for k in 0..bins {
                    let a = complex[k].norm();
                    mag[k] = (mag[k] * n_done + a) / (n_done + 1.0);
                    let cur = complex[k].arg();
                    let nominal = phase_step * k as f32;
                    let mut dev = cur - prev[k] - nominal;
                    dev -= (dev / (2.0 * std::f32::consts::PI)).round() * 2.0 * std::f32::consts::PI;
                    let true_step = nominal + dev;
                    if n_done == 0.0 {
                        ph[k] = cur;
                        om[k] = nominal;
                    } else {
                        // la première mesure de vitesse (trame 1) puis une moyenne glissante
                        om[k] = if n_done == 1.0 { true_step } else { (om[k] * (n_done - 1.0) + true_step) / n_done };
                    }
                }
            }
            // la phase d'analyse de cette trame, gardée AVANT que la synthèse n'écrive dans `complex`
            for k in 0..bins {
                prev[k] = complex[k].arg();
            }
            if active {
                let mag = &mut frozen_mag[channel_idx];
                let ph = &mut phase[channel_idx];
                let om = &omega[channel_idx];
                let level = env * mix;
                for k in 1..bins - 1 {
                    // xorshift32 : une gigue de phase DOUCE par bin et par trame (≤ ±0,25 rad à 100 %),
                    // pour décorréler les bins sans transformer le gel en souffle
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    let jitter = ((rng as f32 / u32::MAX as f32) - 0.5) * smear * 0.5;
                    ph[k] += om[k] + jitter;
                    if ph[k] > 64.0 * std::f32::consts::PI {
                        ph[k] -= 64.0 * std::f32::consts::PI;
                    }
                    let f = k as f32 * bin_hz;
                    let tilt = 10f32.powf(tilt_db_per_oct * (f / 1000.0).max(1e-3).log2() / 20.0);
                    let amp = mag[k] * level * tilt;
                    complex[k] += Complex32::from_polar(amp, ph[k]);
                }
            }

            // la FFT inverse réelle exige des bins DC et Nyquist purement réels
            complex[0].im = 0.0;
            complex[bins - 1].im = 0.0;
            plan.c2r.process(complex, real_fft_buffer).ok();
            for (s, w) in real_fft_buffer.iter_mut().zip(window_fn.iter()) {
                *s *= *w * out_gain;
            }

            // fin de trame (dernier canal) : la capture avance, l'enveloppe s'éteint hors gel
            if channel_idx + 1 == channels {
                if frozen && capture_left > 0 {
                    capture_left -= 1;
                }
                if !frozen {
                    env *= decay_k;
                }
            }
        });

        self.env = env;
        self.capture_left = capture_left;
        self.rng = rng;
        ProcessStatus::Normal
    }
}

impl ClapPlugin for GolemFreeze {
    const CLAP_ID: &'static str = "fm.golem.freeze";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Spectral freeze — hold a chord, play over it");
    const CLAP_MANUAL_URL: Option<&'static str> = Some("https://golem.fm");
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono];
}

impl Vst3Plugin for GolemFreeze {
    const VST3_CLASS_ID: [u8; 16] = *b"GoleMFreezeSpec1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nih_export_clap!(GolemFreeze);
nih_export_vst3!(GolemFreeze);
