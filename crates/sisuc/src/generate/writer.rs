//! The writer of decision 1: every generator rule writes its text once, and the writer puts it
//! into both the program and its rewritten copy. Only a rewrite point, from Task 6 on, writes
//! different text into each.

/// Text written into both copies.
#[derive(Default)]
pub(crate) struct Piece {
    pub(crate) program: String,
    pub(crate) rewritten: String,
}

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) out: Piece,
    /// How many levels the next line is indented.
    indent: usize,
}

impl Writer {
    /// Writes `text` into both copies.
    pub(crate) fn text(&mut self, text: &str) {
        self.out.program.push_str(text);
        self.out.rewritten.push_str(text);
    }

    /// Ends the line and starts the next at the current indentation.
    pub(crate) fn line(&mut self) {
        self.text("\n");
        for _ in 0..self.indent {
            self.text("    ");
        }
    }

    /// Writes `{`, and indents the lines after it.
    pub(crate) fn open(&mut self) {
        self.text("{");
        self.indent += 1;
    }

    /// Writes `}` on a line of its own, one level out.
    pub(crate) fn close(&mut self) {
        self.indent -= 1;
        self.line();
        self.text("}");
    }

    /// Writes into `piece` from now on, and returns what was written so far.
    pub(crate) fn swap(&mut self, piece: Piece) -> Piece {
        std::mem::replace(&mut self.out, piece)
    }

    /// Writes `piece`, which was written elsewhere, into both copies.
    pub(crate) fn paste(&mut self, piece: &Piece) {
        self.out.program.push_str(&piece.program);
        self.out.rewritten.push_str(&piece.rewritten);
    }
}
