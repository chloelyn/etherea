use log::{debug, error, info, trace};
use pixels::{Pixels, SurfaceTexture};
use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::{
        mpsc::{Receiver, TryRecvError},
        Arc, RwLock,
    },
    thread,
};
use winit::{
    dpi::LogicalSize,
    event::VirtualKeyCode,
    event_loop::EventLoop,
    window::{Window, WindowBuilder},
};

mod font;
pub mod input;

macro_rules! wrapper {
    ($($name:ident => $size:expr),*) => {
        $(
            #[derive(Debug)]
            struct $name([u8; $size]);

            impl Default for $name {
                fn default() -> Self {
                    Self([0; $size])
                }
            }

            impl Deref for $name {
                type Target = [u8; $size];

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl DerefMut for $name {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }
        )*
    };
}

pub fn run(intr: Arc<RwLock<Interpreter>>, rx: Receiver<VirtualKeyCode>) {
    Interpreter::main(Arc::clone(&intr), rx);
    Interpreter::timers(intr);
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct Interpreter {
    i: u16,                      // Index register
    pc: u16,                     // Program counter
    stack: Vec<u16>,             // Stack
    memory: Memory,              // Memory
    display: Option<Display>,    // Display
    timers: Arc<RwLock<Timers>>, // Timers
    registers: RegisterArray,    // Variable registers (V0..=VF)
}

impl Interpreter {
    const MEMORY_SIZE: usize = 4096;
    const MEMORY_OFFSET: usize = 0x200;
    const REGISTER_COUNT: usize = 16;

    pub fn new() -> Self {
        Default::default()
    }

    fn main(intr: Arc<RwLock<Interpreter>>, rx: Receiver<VirtualKeyCode>) {
        thread::spawn(move || {
            intr.write().unwrap().execute(rx);
        });
    }

    fn timers(intr: Arc<RwLock<Interpreter>>) {
        let timers = intr.read().unwrap().get_timers();
        thread::spawn(move || loop {
            timers.write().unwrap().update();
            std::thread::sleep(std::time::Duration::from_millis(1000 / 60));
        });
    }

    pub fn attach_display(&mut self, display: Display) {
        self.display = Some(display);
        info!("Attached display [success: true]");
    }

    pub fn load_rom(&mut self, rom: Vec<u8>) {
        self.i = 0;
        self.pc = u16::try_from(Self::MEMORY_OFFSET).unwrap();
        self.stack = Vec::new();
        self.memory = Memory::default();
        self.timers = Arc::new(RwLock::new(Timers::default()));
        self.registers = RegisterArray::default();

        self.memory[font::MEMORY_RANGE].copy_from_slice(font::FONT);
        self.memory[Self::MEMORY_OFFSET..Self::MEMORY_OFFSET + rom.len()].copy_from_slice(&rom);
        info!("Loaded ROM [size: {}]", rom.len());
    }

    fn get_timers(&self) -> Arc<RwLock<Timers>> {
        Arc::clone(&self.timers)
    }

    pub fn get_display_mut(&mut self) -> &mut Display {
        match self.display.as_mut() {
            Some(display) => display,
            None => {
                error!("No display attached");
                std::process::exit(1)
            }
        }
    }

    fn fetch(&mut self) -> u16 {
        let inst = u16::from_be_bytes([
            self.memory[self.pc as usize],
            self.memory[self.pc as usize + 1],
        ]);
        self.pc += 2;
        inst
    }

    fn decode(&mut self) -> Instruction {
        Instruction::from(self.fetch())
    }

    pub fn execute(&mut self, rx: Receiver<VirtualKeyCode>) {
        loop {
            let inst = self.decode();
            debug!("Processing instruction [{:?}]", inst);
            trace!(
                "Timers: [sound: {}] [delay: {}]",
                self.timers.read().unwrap().sound,
                self.timers.read().unwrap().delay
            );
            match inst.nibbles[..] {
                [0, 0, 0xE, 0] => self.get_display_mut().clear(),
                [1, n1, n2, n3] => self.jump(n1, n2, n3),
                [6, register, n1, n2] => self.set_register(register as usize, n1, n2),
                [7, register, n1, n2] => self.add_to_register(register as usize, n1, n2),
                [0xA, n1, n2, n3] => self.set_memory_ptr(n1, n2, n3),
                [0xD, vx, vy, height] => self.draw_sprite(vx as usize, vy as usize, height),
                [0xF, vx, 0x0, 0xA] => self.get_key(vx as usize, &rx),
                [0xE, vx, 0x9, 0xE] => self.skip_key(vx as usize, &rx, true),
                [0xE, vx, 0xA, 0x1] => self.skip_key(vx as usize, &rx, false),
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(1000 / 700));
        }
    }

    fn jump(&mut self, n1: u8, n2: u8, n3: u8) {
        let pc = u16::from_be_bytes([n1, bits::recombine(n2, n3)]);
        self.pc = pc;
        trace!("Jumped PC to {pc}");
    }

    fn set_register(&mut self, register: usize, n1: u8, n2: u8) {
        let value = bits::recombine(n1, n2);
        self.registers[register] = value;
        trace!("Set register V{register:01X} to {value}");
    }

    fn add_to_register(&mut self, register: usize, n1: u8, n2: u8) {
        let value = bits::recombine(n1, n2);
        self.registers[register] += value;
        trace!("Added {value} to register V{register:01X}");
    }

    fn set_memory_ptr(&mut self, n1: u8, n2: u8, n3: u8) {
        let value = u16::from_be_bytes([n1, bits::recombine(n2, n3)]);
        self.i = value;
        trace!("Set index register I to {value}");
    }

    fn draw_sprite(&mut self, vx: usize, vy: usize, height: u8) {
        let x = self.registers[vx] % WIDTH as u8;
        let y = self.registers[vy] % HEIGHT as u8;
        trace!("x: {x} height: {height}");
        self.registers[0xF] = 0;
        for (idx, y) in (y..y + height).enumerate() {
            let sprite = self.memory[self.i as usize..][idx];
            for (n, x) in (x..x + 8).enumerate() {
                let lit = bits::set(7 - n as u8, sprite);
                trace!("Drawing pixel [on: {}] [idx: {idx}] at ({x}, {y})", lit);
                if self
                    .get_display_mut()
                    .write_at(x, y, [0xFF, 0xFF, 0xFF, 0xFF], lit)
                {
                    self.registers[0xF] = 1;
                }
            }
        }
        self.get_display_mut().render();
    }

    fn get_key(&mut self, vx: usize, rx: &Receiver<VirtualKeyCode>) {
        'wait: loop {
            match rx.try_recv() {
                Ok(key) => {
                    let &key = input::KEYMAP.get(&key).unwrap();
                    self.registers[vx] = key;
                    trace!("Stored key {key:01X} in register V{vx:01X}");
                    break 'wait;
                }
                Err(e) => match e {
                    TryRecvError::Empty => {}
                    TryRecvError::Disconnected => {
                        error!("Key receiver hung up");
                        std::process::exit(1);
                    }
                },
            }
        }
    }

    fn skip_key(&mut self, vx: usize, rx: &Receiver<VirtualKeyCode>, press: bool) {
        std::thread::sleep(std::time::Duration::from_millis(200)); // TODO: figure out a better way
        match rx.try_recv() {
            Ok(key) => {
                let &key = input::KEYMAP.get(&key).unwrap();
                trace!("Key received: {key:01X} | VX: {}", self.registers[vx]);
                if press && self.registers[vx] == key {
                    self.pc += 2;
                    trace!("Incremented PC by 2");
                } else if !press && self.registers[vx] != key {
                    self.pc += 2;
                    trace!("Incremented PC by 2");
                }
            }
            Err(e) => match e {
                TryRecvError::Empty => {}
                TryRecvError::Disconnected => {
                    error!("Key receiver hung up");
                    std::process::exit(1);
                }
            },
        };
    }
}

const WIDTH: usize = 64;
const HEIGHT: usize = 32;

#[derive(Debug)]
pub struct Display {
    scratch_pixels: [u8; WIDTH * HEIGHT * 4], // RGBA
    _window: Window,                          // Keeps the window alive
    pixels: Pixels,
}

impl Display {
    pub fn new(event_loop: &EventLoop<()>) -> Self {
        let window = {
            let size = LogicalSize::new(WIDTH as u32, HEIGHT as u32);
            let scaled_size = LogicalSize::new(WIDTH as f64 * 10.0, HEIGHT as f64 * 10.0);
            WindowBuilder::new()
                .with_title("CHIP-8")
                .with_inner_size(scaled_size)
                .with_min_inner_size(size)
                .build(event_loop)
                .unwrap()
        };

        let pixels = {
            let size = window.inner_size();
            let texture = SurfaceTexture::new(size.width, size.height, &window);
            Pixels::new(WIDTH as u32, HEIGHT as u32, texture).unwrap()
        };

        Self {
            scratch_pixels: [0; WIDTH * HEIGHT * 4],
            _window: window,
            pixels,
        }
    }

    fn clear(&mut self) {
        self.scratch_pixels = [0; WIDTH * HEIGHT * 4];
        self.render();
    }

    pub fn render(&mut self) {
        self.draw();
        self.pixels.render().unwrap();
    }

    fn draw(&mut self) {
        let frame = self.pixels.get_frame_mut();
        for (pixel, scratch_pixel) in frame
            .chunks_exact_mut(4)
            .zip(self.scratch_pixels.chunks_exact(4))
        {
            pixel.copy_from_slice(scratch_pixel);
        }
    }

    fn write_at(&mut self, x: u8, y: u8, rgba: [u8; 4], on: bool) -> bool {
        let x = x as usize;
        let y = y as usize;
        let idx = (y * WIDTH + x) * 4;
        let pixels = if on { rgba } else { [0x0, 0x0, 0x0, 0x0] };
        let set = self.scratch_pixels[idx..idx + 4] == [0xFF, 0xFF, 0xFF, 0xFF];
        self.scratch_pixels[idx..idx + 4].copy_from_slice(&pixels);
        set
    }
}

#[derive(Debug)]
pub struct Timers {
    delay: u8,
    sound: u8,
}

impl Timers {
    pub fn update(&mut self) {
        if self.delay > 0 {
            self.delay -= 1;
        }
        if self.sound > 0 {
            self.sound -= 1;
            // TODO: play sound
        }
        trace!(
            "Updated timers: [sound: {}] [delay: {}]",
            self.sound,
            self.delay
        );
    }
}

impl Default for Timers {
    fn default() -> Self {
        Self { delay: 0, sound: 0 }
    }
}

wrapper! {
    Memory => Interpreter::MEMORY_SIZE,
    RegisterArray => Interpreter::REGISTER_COUNT
}

#[derive(PartialEq)]
struct Instruction {
    nibbles: Vec<u8>,
}

impl From<u16> for Instruction {
    fn from(inst: u16) -> Self {
        Self {
            nibbles: inst
                .to_be_bytes()
                .iter()
                .flat_map(|b| vec![(b & 0xF0) >> 4, (b & 0xF)])
                .collect(),
        }
    }
}

impl fmt::Debug for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for nibble in self.nibbles.iter() {
            write!(f, "{:X}", nibble)?;
        }
        Ok(())
    }
}

mod bits {
    pub const fn set(n: u8, bits: u8) -> bool {
        (bits & (1 << n)) != 0
    }

    pub const fn recombine(upper: u8, lower: u8) -> u8 {
        (upper << 4) | lower
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction() {
        let val = 0b00101110; // 46
        let inst = Instruction::from(val);
        assert_eq!(
            inst,
            Instruction {
                nibbles: vec![0, 0, 0b0010, 0b1110]
            }
        );
    }
}
