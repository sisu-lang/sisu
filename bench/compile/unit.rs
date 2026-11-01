// One copy of the program that scripts/compile_speed.py repeats. The generator puts the
// number of the copy in each name that ends in the placeholder, and in one string, so that no
// compiler can merge the copies.
// unit.sisu, unit.cpp and unit.go hold the same program.
use std::fmt::Display;

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
    let label = format!("tally XX {}/{}", tally.count, tally.sum);
    total + label.len() as i64
}
