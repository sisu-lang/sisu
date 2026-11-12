// One copy of the program that scripts/compile_speed.py repeats. The generator
// puts the number of the copy in each name that ends in the placeholder. It
// also puts the number in one string, so that rustc cannot merge the `run`
// functions of the copies.
// unit.sisu, unit.rs and unit.go hold the same program.
// No header is included: a standard header would add the library to what the
// compiler parses.
extern "C" int printf(const char *, ...);

// A string of at most 63 bytes. Every string of the program is shorter.
struct Str {
  char b[64];
  int n;
  Str() : n(0) {}
  Str(const char *s) : n(0) {
    while (s[n]) {
      b[n] = s[n];
      n++;
    }
  }
  long long len() const { return n; }
};

Str operator+(Str a, Str c) {
  for (int i = 0; i < c.n; i++)
    a.b[a.n++] = c.b[i];
  return a;
}

bool operator<(const Str &a, const Str &c) {
  int m = a.n < c.n ? a.n : c.n;
  for (int i = 0; i < m; i++)
    if (a.b[i] != c.b[i])
      return a.b[i] < c.b[i];
  return a.n < c.n;
}

// The decimal text of a number that is not negative.
Str str(long long v) {
  char t[24];
  int k = 0;
  do {
    t[k++] = (char)('0' + v % 10);
    v /= 10;
  } while (v > 0);
  Str s;
  while (k > 0)
    s.b[s.n++] = t[--k];
  return s;
}

Str str(Str s) { return s; }

template <class T> struct Opt {
  bool has = false;
  T val{};
};

template <class T> Opt<T> some(T v) {
  Opt<T> o;
  o.has = true;
  o.val = v;
  return o;
}

template <class T>
concept Ord = requires(T a, T b) { a < b; };

// A value of 100 `long long` fields: 800 bytes. `wide` builds it, copies it,
// writes a field of the copy, reads every field and compares the two.
struct Wide {
  long long f0;
  long long f1;
  long long f2;
  long long f3;
  long long f4;
  long long f5;
  long long f6;
  long long f7;
  long long f8;
  long long f9;
  long long f10;
  long long f11;
  long long f12;
  long long f13;
  long long f14;
  long long f15;
  long long f16;
  long long f17;
  long long f18;
  long long f19;
  long long f20;
  long long f21;
  long long f22;
  long long f23;
  long long f24;
  long long f25;
  long long f26;
  long long f27;
  long long f28;
  long long f29;
  long long f30;
  long long f31;
  long long f32;
  long long f33;
  long long f34;
  long long f35;
  long long f36;
  long long f37;
  long long f38;
  long long f39;
  long long f40;
  long long f41;
  long long f42;
  long long f43;
  long long f44;
  long long f45;
  long long f46;
  long long f47;
  long long f48;
  long long f49;
  long long f50;
  long long f51;
  long long f52;
  long long f53;
  long long f54;
  long long f55;
  long long f56;
  long long f57;
  long long f58;
  long long f59;
  long long f60;
  long long f61;
  long long f62;
  long long f63;
  long long f64;
  long long f65;
  long long f66;
  long long f67;
  long long f68;
  long long f69;
  long long f70;
  long long f71;
  long long f72;
  long long f73;
  long long f74;
  long long f75;
  long long f76;
  long long f77;
  long long f78;
  long long f79;
  long long f80;
  long long f81;
  long long f82;
  long long f83;
  long long f84;
  long long f85;
  long long f86;
  long long f87;
  long long f88;
  long long f89;
  long long f90;
  long long f91;
  long long f92;
  long long f93;
  long long f94;
  long long f95;
  long long f96;
  long long f97;
  long long f98;
  long long f99;
  bool operator==(const Wide &) const = default;
};

long long wide(long long base) {
  Wide w{
      base,      base + 1,  base + 2,  base + 3,  base + 4,  base + 5,
      base + 6,  base + 7,  base + 8,  base + 9,  base + 10, base + 11,
      base + 12, base + 13, base + 14, base + 15, base + 16, base + 17,
      base + 18, base + 19, base + 20, base + 21, base + 22, base + 23,
      base + 24, base + 25, base + 26, base + 27, base + 28, base + 29,
      base + 30, base + 31, base + 32, base + 33, base + 34, base + 35,
      base + 36, base + 37, base + 38, base + 39, base + 40, base + 41,
      base + 42, base + 43, base + 44, base + 45, base + 46, base + 47,
      base + 48, base + 49, base + 50, base + 51, base + 52, base + 53,
      base + 54, base + 55, base + 56, base + 57, base + 58, base + 59,
      base + 60, base + 61, base + 62, base + 63, base + 64, base + 65,
      base + 66, base + 67, base + 68, base + 69, base + 70, base + 71,
      base + 72, base + 73, base + 74, base + 75, base + 76, base + 77,
      base + 78, base + 79, base + 80, base + 81, base + 82, base + 83,
      base + 84, base + 85, base + 86, base + 87, base + 88, base + 89,
      base + 90, base + 91, base + 92, base + 93, base + 94, base + 95,
      base + 96, base + 97, base + 98, base + 99,
  };
  Wide before = w;
  w.f0 += 1;
  long long total = 0;
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
  if (w != before)
    total += 2;
  return total;
}

template <class T> struct NodeXX {
  T value;
  NodeXX<T> *next;
};

template <class T> struct StackXX {
  NodeXX<T> *top = nullptr;
  long long size = 0;

  void push(T x) {
    top = new NodeXX<T>{x, top};
    size++;
  }

  Opt<T> pop() {
    if (!top)
      return Opt<T>{};
    NodeXX<T> *node = top;
    top = node->next;
    size--;
    T v = node->value;
    delete node;
    return some(v);
  }
};

template <class U> struct SourceXX {
  using Unit = U;
  virtual Opt<U> next() = 0;
  virtual ~SourceXX() {}
};

struct CountdownXX : SourceXX<long long> {
  long long left;
  CountdownXX(long long l) : left(l) {}
  Opt<long long> next() override {
    if (left == 0)
      return Opt<long long>{};
    left--;
    return some(left + 1);
  }
};

struct LettersXX : SourceXX<Str> {
  long long taken;
  long long limit;
  LettersXX(long long t, long long l) : taken(t), limit(l) {}
  Opt<Str> next() override {
    if (taken == limit)
      return Opt<Str>{};
    taken++;
    return some(Str("x") + str(taken));
  }
};

template <class S> long long drainXX(S &s) {
  long long total = 0;
  for (Opt<typename S::Unit> x = s.next(); x.has; x = s.next())
    total = total * 3 + str(x.val).len();
  return total;
}

template <Ord T, int N> Opt<T> largestXX(T (&xs)[N]) {
  Opt<T> best;
  for (int i = 0; i < N; i++)
    if (!best.has || best.val < xs[i])
      best = some(xs[i]);
  return best;
}

struct ShapeXX {
  enum Kind { Dot, Line, Rect } kind;
  long long a;
  long long b;
};

long long areaXX(ShapeXX s) {
  switch (s.kind) {
  case ShapeXX::Dot:
    return 1;
  case ShapeXX::Line:
    return s.a;
  case ShapeXX::Rect:
    return s.a * s.b;
  }
  return 0;
}

struct TallyXX {
  long long count = 0;
  long long sum = 0;

  void add(long long n) {
    count++;
    sum += n;
  }

  Opt<long long> mean() {
    if (count == 0)
      return Opt<long long>{};
    return some(sum / count);
  }
};

long long runXX() {
  StackXX<long long> ints;
  StackXX<Str> words;
  for (long long i = 0; i < 4; i++) {
    ints.push(i * 3 + 1);
    words.push(Str("w") + str(i));
  }
  long long total = 0;
  for (Opt<long long> v = ints.pop(); v.has; v = ints.pop())
    total = total * 2 + v.val;
  for (Opt<Str> w = words.pop(); w.has; w = words.pop())
    total += w.val.len();
  total += ints.size + words.size;
  CountdownXX countdown(5);
  total += drainXX(countdown);
  LettersXX letters(0, 4);
  total += drainXX(letters);
  long long nums[] = {3, 9, 2};
  total += largestXX(nums).val;
  Str strs[] = {"b", "ccc", "a"};
  total += largestXX(strs).val.len();
  unsigned char bytes[] = {4, 8, 1};
  total += (long long)largestXX(bytes).val;
  TallyXX tally;
  ShapeXX shapes[] = {
      {ShapeXX::Dot, 0, 0}, {ShapeXX::Line, 4, 0}, {ShapeXX::Rect, 3, 5}};
  for (ShapeXX s : shapes)
    tally.add(areaXX(s));
  total += tally.mean().val;
  total += wide(XX);
  Str label = Str("tally XX ") + str(tally.count) + Str("/") + str(tally.sum);
  return total + label.len();
}
