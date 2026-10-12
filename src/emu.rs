//! Frontend libretro mínimo: corre el núcleo (snes9x) en un hilo propio.

use eframe::egui::{self, Color32, ColorImage};
use libretro_sys as rs;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{CString, c_char, c_uint, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

/// Botones pulsados (bit = DEVICE_ID_JOYPAD_*): mando y teclado.
pub static PAD: AtomicU32 = AtomicU32::new(0);
pub static KEYS: AtomicU32 = AtomicU32::new(0);

pub const B: u32 = 0;
pub const Y: u32 = 1;
pub const SELECT: u32 = 2;
pub const START: u32 = 3;
pub const UP: u32 = 4;
pub const DOWN: u32 = 5;
pub const LEFT: u32 = 6;
pub const RIGHT: u32 = 7;
pub const A: u32 = 8;
pub const X: u32 = 9;
pub const L: u32 = 10;
pub const R: u32 = 11;

type Audio = Arc<Mutex<VecDeque<[f32; 2]>>>;
type Frame = Arc<Mutex<Option<ColorImage>>>;

/// Estado que necesitan los callbacks C (que no reciben puntero de usuario).
struct Cb {
    fmt: u32,
    frame: Frame,
    audio: Audio,
    dir: CString,
    ctx: egui::Context,
}

thread_local! {
    static CB: RefCell<Option<Cb>> = const { RefCell::new(None) };
}

unsafe extern "C" fn env(cmd: c_uint, data: *mut c_void) -> bool {
    CB.with_borrow_mut(|cb| {
        let Some(cb) = cb else { return false };
        // SAFETY: el tipo de `data` lo define cada comando de la API libretro.
        unsafe {
            match cmd {
                rs::ENVIRONMENT_SET_PIXEL_FORMAT => {
                    cb.fmt = *(data as *const u32);
                    cb.fmt <= 2
                }
                rs::ENVIRONMENT_GET_SYSTEM_DIRECTORY | rs::ENVIRONMENT_GET_SAVE_DIRECTORY => {
                    *(data as *mut *const c_char) = cb.dir.as_ptr();
                    true
                }
                rs::ENVIRONMENT_GET_CAN_DUPE => {
                    *(data as *mut bool) = true;
                    true
                }
                _ => false,
            }
        }
    })
}

fn expand5(v: u32) -> u8 {
    (v << 3 | v >> 2) as u8
}

unsafe extern "C" fn video(data: *const c_void, w: c_uint, h: c_uint, pitch: usize) {
    if data.is_null() {
        return; // frame duplicado
    }
    CB.with_borrow(|cb| {
        let Some(cb) = cb else { return };
        let (w, h) = (w as usize, h as usize);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            // SAFETY: el núcleo garantiza `h` filas de `pitch` bytes.
            let row = unsafe { (data as *const u8).add(y * pitch) };
            for x in 0..w {
                px.push(if cb.fmt == 1 {
                    let p = unsafe { *(row as *const u32).add(x) };
                    Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
                } else {
                    let p = unsafe { *(row as *const u16).add(x) } as u32;
                    if cb.fmt == 2 {
                        let g = p >> 5 & 63;
                        Color32::from_rgb(expand5(p >> 11 & 31), (g << 2 | g >> 4) as u8, expand5(p & 31))
                    } else {
                        Color32::from_rgb(expand5(p >> 10 & 31), expand5(p >> 5 & 31), expand5(p & 31))
                    }
                });
            }
        }
        *cb.frame.lock().unwrap() = Some(ColorImage::new([w, h], px));
        cb.ctx.request_repaint();
    })
}

unsafe extern "C" fn audio_batch(data: *const i16, frames: usize) -> usize {
    CB.with_borrow(|cb| {
        let Some(cb) = cb else { return };
        // SAFETY: `frames` muestras estéreo intercaladas.
        let s = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        cb.audio.lock().unwrap().extend(s.chunks_exact(2).map(|c| [c[0] as f32 / 32768., c[1] as f32 / 32768.]));
    });
    frames
}

unsafe extern "C" fn audio_sample(l: i16, r: i16) {
    unsafe { audio_batch([l, r].as_ptr(), 1) };
}

unsafe extern "C" fn input_poll() {}

unsafe extern "C" fn input_state(port: c_uint, device: c_uint, _index: c_uint, id: c_uint) -> i16 {
    if port != 0 || device != rs::DEVICE_JOYPAD || id > 15 {
        return 0;
    }
    ((PAD.load(Relaxed) | KEYS.load(Relaxed)) >> id & 1) as i16
}

/// Abre la salida de audio; reescala de la frecuencia del núcleo a la del dispositivo.
fn open_audio(q: Audio, in_rate: f64) -> Option<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let dev = cpal::default_host().default_output_device()?;
    let cfg = dev.default_output_config().ok()?.config();
    let (ch, step) = (cfg.channels as usize, in_rate / cfg.sample_rate as f64);
    let (mut pos, mut cur) = (0.0, [0f32; 2]);
    let stream = dev
        .build_output_stream::<f32, _, _>(
            cfg,
            move |out: &mut [f32], _| {
                let mut q = q.lock().unwrap();
                for f in out.chunks_mut(ch) {
                    // ponytail: remuestreo por retención; interpolar si se oye áspero
                    pos += step;
                    while pos >= 1.0 {
                        pos -= 1.0;
                        cur = q.pop_front().unwrap_or([0.; 2]);
                    }
                    for (i, o) in f.iter_mut().enumerate() {
                        *o = cur[i.min(1)];
                    }
                }
            },
            |e| eprintln!("audio: {e}"),
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(stream)
}

pub struct Emu {
    pub frame: Frame,
    pub aspect: f32,
    paused: Arc<AtomicBool>,
    reset: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Emu {
    pub fn start(core: PathBuf, rom: PathBuf, ctx: egui::Context) -> Result<Emu, String> {
        let frame = Frame::default();
        let [paused, reset, stop] = [(); 3].map(|_| Arc::new(AtomicBool::new(false)));
        let (tx, rx) = mpsc::channel();
        let flags = (frame.clone(), paused.clone(), reset.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            if let Err(e) = run(&core, &rom, ctx, flags, &tx) {
                tx.send(Err(e)).ok();
            }
        });
        let aspect = rx.recv().map_err(|_| crate::t("El emulador se cerró inesperadamente", "The emulator closed unexpectedly").to_string())??;
        Ok(Emu { frame, aspect, paused, reset, stop, thread: Some(thread) })
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Relaxed);
    }

    pub fn reset(&self) {
        self.reset.store(true, Relaxed);
    }
}

impl Drop for Emu {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

macro_rules! sym {
    ($lib:ident, $name:literal: $($t:tt)*) => {
        // SAFETY: firmas de la API libretro v1.
        *unsafe { $lib.get::<unsafe extern "C" fn $($t)*>(concat!($name, "\0").as_bytes()) }
            .map_err(|e| format!("{} ({}): {e}", crate::t("Núcleo inválido", "Invalid core"), $name))?
    };
}

type Flags = (Frame, Arc<AtomicBool>, Arc<AtomicBool>, Arc<AtomicBool>);

fn run(core: &Path, rom: &Path, ctx: egui::Context, flags: Flags, tx: &mpsc::Sender<Result<f32, String>>) -> Result<(), String> {
    let (frame, paused, reset, stop) = flags;
    // SAFETY: cargar el núcleo ejecuta sus constructores; es la librería que eligió el usuario.
    let lib = unsafe { libloading::Library::new(core) }
        .map_err(|e| format!("{} {}: {e}", crate::t("No se pudo cargar el núcleo", "Could not load the core"), core.display()))?;
    let set_env = sym!(lib, "retro_set_environment": (rs::EnvironmentFn));
    let set_video = sym!(lib, "retro_set_video_refresh": (rs::VideoRefreshFn));
    let set_sample = sym!(lib, "retro_set_audio_sample": (rs::AudioSampleFn));
    let set_batch = sym!(lib, "retro_set_audio_sample_batch": (rs::AudioSampleBatchFn));
    let set_poll = sym!(lib, "retro_set_input_poll": (rs::InputPollFn));
    let set_input = sym!(lib, "retro_set_input_state": (rs::InputStateFn));
    let init = sym!(lib, "retro_init": ());
    let deinit = sym!(lib, "retro_deinit": ());
    let load_game = sym!(lib, "retro_load_game": (*const rs::GameInfo) -> bool);
    let unload_game = sym!(lib, "retro_unload_game": ());
    let av_info = sym!(lib, "retro_get_system_av_info": (*mut rs::SystemAvInfo));
    let retro_run = sym!(lib, "retro_run": ());
    let retro_reset = sym!(lib, "retro_reset": ());
    let mem_data = sym!(lib, "retro_get_memory_data": (c_uint) -> *mut c_void);
    let mem_size = sym!(lib, "retro_get_memory_size": (c_uint) -> usize);

    let audio = Audio::default();
    let dir = CString::new(rom.parent().unwrap_or(Path::new(".")).as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    CB.set(Some(Cb { fmt: 0, frame, audio: audio.clone(), dir, ctx }));
    let data = std::fs::read(rom).map_err(|e| format!("{}: {e}", crate::t("No se pudo leer la ROM", "Could not read the ROM")))?;
    let path = CString::new(rom.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let info = rs::GameInfo { path: path.as_ptr(), data: data.as_ptr() as _, size: data.len(), meta: std::ptr::null() };
    // SAFETY: secuencia estándar de arranque libretro, todo en este hilo.
    let mut av: rs::SystemAvInfo = unsafe {
        set_env(env);
        set_video(video);
        set_sample(audio_sample);
        set_batch(audio_batch);
        set_poll(input_poll);
        set_input(input_state);
        init();
        if !load_game(&info) {
            deinit();
            return Err(crate::t("El núcleo no pudo cargar la ROM", "The core could not load the ROM").into());
        }
        std::mem::zeroed()
    };
    unsafe { av_info(&mut av) };

    // Partidas guardadas (SRAM) junto a la ROM, como .srm.
    let srm = rom.with_extension("srm");
    let sram = || -> &'static mut [u8] {
        let (p, n) = unsafe { (mem_data(rs::MEMORY_SAVE_RAM) as *mut u8, mem_size(rs::MEMORY_SAVE_RAM)) };
        if p.is_null() { &mut [] } else { unsafe { std::slice::from_raw_parts_mut(p, n) } }
    };
    let save = || {
        let s = sram();
        if !s.is_empty() && let Err(e) = std::fs::write(&srm, &*s) {
            eprintln!("No se pudo guardar {}: {e}", srm.display());
        }
    };
    if let Ok(saved) = std::fs::read(&srm) {
        let s = sram();
        let n = s.len().min(saved.len());
        s[..n].copy_from_slice(&saved[..n]);
    }

    let g = &av.geometry;
    tx.send(Ok(if g.aspect_ratio > 0. { g.aspect_ratio } else { 4. / 3. })).ok();
    let stream = open_audio(audio.clone(), av.timing.sample_rate);
    let frame_time = Duration::from_secs_f64(1. / av.timing.fps);
    let max_queue = (av.timing.sample_rate * 0.06) as usize;
    let (mut next, mut was_paused) = (Instant::now(), false);
    while !stop.load(Relaxed) {
        if paused.load(Relaxed) {
            if !was_paused {
                save();
                audio.lock().unwrap().clear();
            }
            was_paused = true;
            sleep(Duration::from_millis(16));
            continue;
        }
        if was_paused {
            next = Instant::now();
            was_paused = false;
        }
        if reset.swap(false, Relaxed) {
            unsafe { retro_reset() };
        }
        unsafe { retro_run() };
        if stream.is_some() {
            // El reloj del audio marca la velocidad.
            while audio.lock().unwrap().len() > max_queue && !stop.load(Relaxed) {
                sleep(Duration::from_millis(1));
            }
        } else {
            next += frame_time;
            match next.checked_duration_since(Instant::now()) {
                Some(d) => sleep(d),
                None => next = Instant::now(),
            }
        }
    }
    save();
    drop(stream);
    unsafe {
        unload_game();
        deinit();
    }
    CB.set(None);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Corre el núcleo de verdad: `SMW_ROM=/ruta/smw.sfc SMW_CORE=/ruta/snes9x_libretro.so cargo test -- --ignored`
    #[test]
    #[ignore]
    fn runs_core() {
        let dir = std::env::temp_dir().join("smw-emu-test");
        std::fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("smw.sfc");
        std::fs::copy(std::env::var("SMW_ROM").unwrap(), &rom).unwrap();
        let emu = super::Emu::start(std::env::var("SMW_CORE").unwrap().into(), rom.clone(), Default::default()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let img = emu.frame.lock().unwrap().take().expect("sin video");
        assert_eq!(img.size, [256, 224]);
        drop(emu);
        assert!(rom.with_extension("srm").is_file(), "no guardó la SRAM");
        std::fs::remove_dir_all(dir).ok();
    }
}
