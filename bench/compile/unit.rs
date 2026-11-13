// One copy of the program that scripts/compile_speed.py repeats. The generator puts the
// number of the copy in each name that ends in the placeholder. It also puts the number in
// one string, so that rustc cannot merge the `run` functions of the copies.
// unit.sisu, unit.cpp and unit.go hold the same program.
use std::fmt::Display;

// A value of 100 `i64` fields: 800 bytes. `wide` builds it, copies it, writes a field of the
// copy, reads every field and compares the two.
#[derive(Clone, Copy, PartialEq)]
struct Wide {
    f0: i64,
    f1: i64,
    f2: i64,
    f3: i64,
    f4: i64,
    f5: i64,
    f6: i64,
    f7: i64,
    f8: i64,
    f9: i64,
    f10: i64,
    f11: i64,
    f12: i64,
    f13: i64,
    f14: i64,
    f15: i64,
    f16: i64,
    f17: i64,
    f18: i64,
    f19: i64,
    f20: i64,
    f21: i64,
    f22: i64,
    f23: i64,
    f24: i64,
    f25: i64,
    f26: i64,
    f27: i64,
    f28: i64,
    f29: i64,
    f30: i64,
    f31: i64,
    f32: i64,
    f33: i64,
    f34: i64,
    f35: i64,
    f36: i64,
    f37: i64,
    f38: i64,
    f39: i64,
    f40: i64,
    f41: i64,
    f42: i64,
    f43: i64,
    f44: i64,
    f45: i64,
    f46: i64,
    f47: i64,
    f48: i64,
    f49: i64,
    f50: i64,
    f51: i64,
    f52: i64,
    f53: i64,
    f54: i64,
    f55: i64,
    f56: i64,
    f57: i64,
    f58: i64,
    f59: i64,
    f60: i64,
    f61: i64,
    f62: i64,
    f63: i64,
    f64: i64,
    f65: i64,
    f66: i64,
    f67: i64,
    f68: i64,
    f69: i64,
    f70: i64,
    f71: i64,
    f72: i64,
    f73: i64,
    f74: i64,
    f75: i64,
    f76: i64,
    f77: i64,
    f78: i64,
    f79: i64,
    f80: i64,
    f81: i64,
    f82: i64,
    f83: i64,
    f84: i64,
    f85: i64,
    f86: i64,
    f87: i64,
    f88: i64,
    f89: i64,
    f90: i64,
    f91: i64,
    f92: i64,
    f93: i64,
    f94: i64,
    f95: i64,
    f96: i64,
    f97: i64,
    f98: i64,
    f99: i64,
}

fn wide(base: i64) -> i64 {
    let mut w = Wide {
        f0: base,
        f1: base + 1,
        f2: base + 2,
        f3: base + 3,
        f4: base + 4,
        f5: base + 5,
        f6: base + 6,
        f7: base + 7,
        f8: base + 8,
        f9: base + 9,
        f10: base + 10,
        f11: base + 11,
        f12: base + 12,
        f13: base + 13,
        f14: base + 14,
        f15: base + 15,
        f16: base + 16,
        f17: base + 17,
        f18: base + 18,
        f19: base + 19,
        f20: base + 20,
        f21: base + 21,
        f22: base + 22,
        f23: base + 23,
        f24: base + 24,
        f25: base + 25,
        f26: base + 26,
        f27: base + 27,
        f28: base + 28,
        f29: base + 29,
        f30: base + 30,
        f31: base + 31,
        f32: base + 32,
        f33: base + 33,
        f34: base + 34,
        f35: base + 35,
        f36: base + 36,
        f37: base + 37,
        f38: base + 38,
        f39: base + 39,
        f40: base + 40,
        f41: base + 41,
        f42: base + 42,
        f43: base + 43,
        f44: base + 44,
        f45: base + 45,
        f46: base + 46,
        f47: base + 47,
        f48: base + 48,
        f49: base + 49,
        f50: base + 50,
        f51: base + 51,
        f52: base + 52,
        f53: base + 53,
        f54: base + 54,
        f55: base + 55,
        f56: base + 56,
        f57: base + 57,
        f58: base + 58,
        f59: base + 59,
        f60: base + 60,
        f61: base + 61,
        f62: base + 62,
        f63: base + 63,
        f64: base + 64,
        f65: base + 65,
        f66: base + 66,
        f67: base + 67,
        f68: base + 68,
        f69: base + 69,
        f70: base + 70,
        f71: base + 71,
        f72: base + 72,
        f73: base + 73,
        f74: base + 74,
        f75: base + 75,
        f76: base + 76,
        f77: base + 77,
        f78: base + 78,
        f79: base + 79,
        f80: base + 80,
        f81: base + 81,
        f82: base + 82,
        f83: base + 83,
        f84: base + 84,
        f85: base + 85,
        f86: base + 86,
        f87: base + 87,
        f88: base + 88,
        f89: base + 89,
        f90: base + 90,
        f91: base + 91,
        f92: base + 92,
        f93: base + 93,
        f94: base + 94,
        f95: base + 95,
        f96: base + 96,
        f97: base + 97,
        f98: base + 98,
        f99: base + 99,
    };
    let before = w;
    w.f0 += 1;
    let mut total = 0;
    total += w.f0;
    total += w.f1;
    total += w.f2;
    total += w.f3;
    total += w.f4;
    total += w.f5;
    total += w.f6;
    total += w.f7;
    total += w.f8;
    total += w.f9;
    total += w.f10;
    total += w.f11;
    total += w.f12;
    total += w.f13;
    total += w.f14;
    total += w.f15;
    total += w.f16;
    total += w.f17;
    total += w.f18;
    total += w.f19;
    total += w.f20;
    total += w.f21;
    total += w.f22;
    total += w.f23;
    total += w.f24;
    total += w.f25;
    total += w.f26;
    total += w.f27;
    total += w.f28;
    total += w.f29;
    total += w.f30;
    total += w.f31;
    total += w.f32;
    total += w.f33;
    total += w.f34;
    total += w.f35;
    total += w.f36;
    total += w.f37;
    total += w.f38;
    total += w.f39;
    total += w.f40;
    total += w.f41;
    total += w.f42;
    total += w.f43;
    total += w.f44;
    total += w.f45;
    total += w.f46;
    total += w.f47;
    total += w.f48;
    total += w.f49;
    total += w.f50;
    total += w.f51;
    total += w.f52;
    total += w.f53;
    total += w.f54;
    total += w.f55;
    total += w.f56;
    total += w.f57;
    total += w.f58;
    total += w.f59;
    total += w.f60;
    total += w.f61;
    total += w.f62;
    total += w.f63;
    total += w.f64;
    total += w.f65;
    total += w.f66;
    total += w.f67;
    total += w.f68;
    total += w.f69;
    total += w.f70;
    total += w.f71;
    total += w.f72;
    total += w.f73;
    total += w.f74;
    total += w.f75;
    total += w.f76;
    total += w.f77;
    total += w.f78;
    total += w.f79;
    total += w.f80;
    total += w.f81;
    total += w.f82;
    total += w.f83;
    total += w.f84;
    total += w.f85;
    total += w.f86;
    total += w.f87;
    total += w.f88;
    total += w.f89;
    total += w.f90;
    total += w.f91;
    total += w.f92;
    total += w.f93;
    total += w.f94;
    total += w.f95;
    total += w.f96;
    total += w.f97;
    total += w.f98;
    total += w.f99;
    if w != before {
        total += 2;
    }
    total
}

struct NodeXX<T> {
    value: T,
    next: Option<Box<NodeXX<T>>>,
}

struct StackXX<T> {
    top: Option<Box<NodeXX<T>>>,
    size: i64,
}

impl<T> StackXX<T> {
    fn push(&mut self, x: T) {
        self.top = Some(Box::new(NodeXX {
            value: x,
            next: self.top.take(),
        }));
        self.size += 1;
    }

    fn pop(&mut self) -> Option<T> {
        self.top.take().map(|node| {
            self.top = node.next;
            self.size -= 1;
            node.value
        })
    }
}

trait SourceXX {
    type Item: Display;
    fn next(&mut self) -> Option<Self::Item>;
}

struct CountdownXX {
    left: i64,
}

impl SourceXX for CountdownXX {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        Some(self.left + 1)
    }
}

struct LettersXX {
    taken: i64,
    limit: i64,
}

impl SourceXX for LettersXX {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        if self.taken == self.limit {
            return None;
        }
        self.taken += 1;
        Some(format!("x{}", self.taken))
    }
}

fn drainXX<S: SourceXX>(s: &mut S) -> i64 {
    let mut total = 0;
    while let Some(x) = s.next() {
        total = total * 3 + x.to_string().len() as i64;
    }
    total
}

fn largestXX<T: PartialOrd>(xs: Vec<T>) -> Option<T> {
    let mut best: Option<T> = None;
    for x in xs {
        if best.as_ref().is_none_or(|b| *b < x) {
            best = Some(x);
        }
    }
    best
}

enum ShapeXX {
    Dot,
    Line(i64),
    Rect(i64, i64),
}

fn areaXX(s: ShapeXX) -> i64 {
    match s {
        ShapeXX::Dot => 1,
        ShapeXX::Line(n) => n,
        ShapeXX::Rect(w, h) => w * h,
    }
}

struct TallyXX {
    count: i64,
    sum: i64,
}

impl TallyXX {
    fn add(&mut self, n: i64) {
        self.count += 1;
        self.sum += n;
    }

    fn mean(&self) -> Option<i64> {
        if self.count == 0 {
            None
        } else {
            Some(self.sum / self.count)
        }
    }
}

fn runXX() -> i64 {
    let mut ints: StackXX<i64> = StackXX { top: None, size: 0 };
    let mut words: StackXX<String> = StackXX { top: None, size: 0 };
    for i in 0..4 {
        ints.push(i * 3 + 1);
        words.push(format!("w{i}"));
    }
    let mut total = 0;
    while let Some(v) = ints.pop() {
        total = total * 2 + v;
    }
    while let Some(w) = words.pop() {
        total += w.len() as i64;
    }
    total += ints.size + words.size;
    total += drainXX(&mut CountdownXX { left: 5 });
    total += drainXX(&mut LettersXX { taken: 0, limit: 4 });
    total += largestXX(vec![3, 9, 2]).unwrap_or(0);
    total += largestXX(vec!["b", "ccc", "a"]).unwrap_or("").len() as i64;
    let bytes: Vec<u8> = vec![4, 8, 1];
    total += i64::from(largestXX(bytes).unwrap_or(0));
    let mut tally = TallyXX { count: 0, sum: 0 };
    let shapes = [ShapeXX::Dot, ShapeXX::Line(4), ShapeXX::Rect(3, 5)];
    for s in shapes {
        tally.add(areaXX(s));
    }
    total += tally.mean().unwrap_or(0);
    total += wide(XX);
    let label = format!("tally XX {}/{}", tally.count, tally.sum);
    total + label.len() as i64
}
