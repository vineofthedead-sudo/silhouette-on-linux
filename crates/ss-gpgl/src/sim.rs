//! A GPGL interpreter: replays a command stream as the cutter would and records what the tool
//! does. It is the reference model for golden tests and the core of the virtual cutter.

use ss_geom::Point;

use crate::{ESC, ETX, from_units};

/// A registration mark search seen in the stream (`TB123` or `TB23`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimRegistration {
    pub auto_search: bool,
    /// Distance between the marks, mm (width across, height down).
    pub size_mm: (f64, f64),
    /// Where the automatic search starts, mm (x, y).
    pub search_from_mm: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Simulation {
    /// Tool-down polylines in millimetres (x across, y down).
    pub strokes: Vec<Vec<Point>>,
    /// Tool-up travel distance in mm.
    pub travel_mm: f64,
    pub force: Option<u32>,
    pub speed: Option<u32>,
    pub blade_offset: Option<u32>,
    pub blade_depth: Option<u32>,
    pub track_enhancing: Option<bool>,
    pub tool_holder: Option<u8>,
    pub homes: usize,
    /// Media size from `FU`, in mm (width, height).
    pub media_mm: Option<(f64, f64)>,
    /// Registration mark arm length (`TB51`) and thickness (`TB53`), mm.
    pub reg_mark_length_mm: Option<f64>,
    pub reg_mark_thickness_mm: Option<f64>,
    /// Mark search; tool coordinates after it are relative to the top-left mark.
    pub registration: Option<SimRegistration>,
    pub homed: bool,
    /// Every command, without ETX; control sequences as `ESC+<hex>`.
    pub commands: Vec<String>,
    /// Commands the interpreter does not model.
    pub unknown: Vec<String>,
    /// Malformed commands.
    pub errors: Vec<String>,
    /// Bytes after the last ETX.
    pub trailing: Vec<u8>,
}

fn numbers(s: &str) -> Option<Vec<f64>> {
    if s.trim().is_empty() {
        return Some(Vec::new());
    }
    s.split(',').map(|v| v.trim().parse::<f64>().ok().filter(|f| f.is_finite())).collect()
}

/// GPGL `y,x` unit pair to a point in mm.
fn pt(y: f64, x: f64) -> Point {
    Point::new(from_units(x), from_units(y))
}

struct Pen {
    pos: Point,
    down: bool,
}

impl Simulation {
    fn move_to(&mut self, pen: &mut Pen, p: Point) {
        self.travel_mm += pen.pos.distance(p);
        pen.pos = p;
        pen.down = false;
    }

    fn draw_to(&mut self, pen: &mut Pen, p: Point) {
        if !pen.down {
            self.strokes.push(vec![pen.pos]);
            pen.down = true;
        }
        if let Some(s) = self.strokes.last_mut() {
            s.push(p);
        }
        pen.pos = p;
    }

    fn command(&mut self, pen: &mut Pen, c: &str) {
        self.commands.push(c.to_string());
        let bad = |s: &mut Self| s.errors.push(c.to_string());
        let first_u32 = |args: &str| numbers(args).and_then(|v| v.first().map(|f| *f as u32));
        // Longest prefixes first.
        const PREFIXES: [&str; 21] =
            ["FN", "FU", "FC", "FY", "FX", "FM", "FO", "FE", "FF", "FG", "TB", "TC", "BZ", "J", "M", "D", "H", "!", "&", "\\", "Z"];
        let Some(p) = PREFIXES.iter().find(|p| c.starts_with(**p)) else {
            self.unknown.push(c.to_string());
            return;
        };
        let args = &c[p.len()..];
        match *p {
            "M" => match numbers(args).as_deref() {
                Some([y, x]) => self.move_to(pen, pt(*y, *x)),
                _ => bad(self),
            },
            "D" => match numbers(args) {
                Some(v) if !v.is_empty() && v.len() % 2 == 0 => {
                    for yx in v.chunks(2) {
                        self.draw_to(pen, pt(yx[0], yx[1]));
                    }
                }
                _ => bad(self),
            },
            "BZ" => match numbers(args).as_deref() {
                Some([mode, c @ ..]) if c.len() == 8 => {
                    let p: Vec<Point> = c.chunks(2).map(|yx| pt(yx[0], yx[1])).collect();
                    if *mode == 0.0 {
                        self.move_to(pen, p[0]);
                    }
                    let cubic = ss_geom::kurbo::CubicBez::new(p[0], p[1], p[2], p[3]);
                    for i in 1..=32 {
                        let q = ss_geom::kurbo::ParamCurve::eval(&cubic, f64::from(i) / 32.0);
                        self.draw_to(pen, q);
                    }
                }
                _ => bad(self),
            },
            "H" => {
                self.move_to(pen, Point::ZERO);
                self.homed = true;
                self.homes += 1;
            }
            "FU" => match numbers(args).as_deref() {
                Some([h, w]) => self.media_mm = Some((from_units(*w), from_units(*h))),
                _ => bad(self),
            },
            "FX" => match first_u32(args) {
                Some(f) => self.force = Some(f),
                None => bad(self),
            },
            "!" => match first_u32(args) {
                Some(v) => self.speed = Some(v),
                None => bad(self),
            },
            "FC" => match first_u32(args) {
                Some(v) => self.blade_offset = Some(v),
                None => bad(self),
            },
            "TC" => match first_u32(args) {
                Some(v) => self.blade_depth = Some(v),
                None => bad(self),
            },
            "FY" => match first_u32(args) {
                Some(v) => self.track_enhancing = Some(v != 0),
                None => bad(self),
            },
            "J" => match first_u32(args) {
                Some(v) => self.tool_holder = u8::try_from(v).ok(),
                None => bad(self),
            },
            "TB" => match numbers(args).as_deref() {
                Some([51.0, len]) => self.reg_mark_length_mm = Some(from_units(*len)),
                Some([53.0, t]) => self.reg_mark_thickness_mm = Some(from_units(*t)),
                Some([123.0, h, w, top, left]) => {
                    self.registration = Some(SimRegistration {
                        auto_search: true,
                        size_mm: (from_units(*w), from_units(*h)),
                        search_from_mm: Some((from_units(*left), from_units(*top))),
                    });
                }
                Some([23.0, h, w]) => {
                    self.registration =
                        Some(SimRegistration { auto_search: false, size_mm: (from_units(*w), from_units(*h)), search_from_mm: None });
                }
                Some([123.0 | 23.0 | 51.0 | 53.0, ..]) | None => bad(self),
                Some(_) => {}
            },
            "FN" | "&" | "\\" | "Z" | "FM" | "FO" | "FE" | "FF" | "FG" => {
                if numbers(args).is_none() {
                    bad(self);
                }
            }
            _ => self.unknown.push(c.to_string()),
        }
    }
}

/// Interprets a GPGL byte stream.
pub fn simulate(bytes: &[u8]) -> Simulation {
    let mut sim = Simulation::default();
    let mut pen = Pen { pos: Point::ZERO, down: false };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ESC {
            match bytes.get(i + 1) {
                Some(b) => sim.commands.push(format!("ESC+{b:02x}")),
                None => sim.errors.push("ESC at end of stream".into()),
            }
            i += 2;
            continue;
        }
        let Some(len) = bytes[i..].iter().position(|b| *b == ETX) else {
            sim.trailing = bytes[i..].to_vec();
            break;
        };
        let cmd = String::from_utf8_lossy(&bytes[i..i + len]).into_owned();
        sim.command(&mut pen, &cmd);
        i += len + 1;
    }
    sim
}
