//! The registry: every diagnostic code, and the message, labels, help and fixes of each.

use super::{Diagnostic, Edit, Fix, Severity, Suggestion};
use crate::source::Span;

/// The code table: each code's variant and number. A number is never reused or changed.
/// A row may name extra `DiagnosticKind` variants after `|` that share its code.
macro_rules! codes {
    ($($variant:ident $(| $alias:ident)* = $code:literal,)*) => {
        /// A diagnostic code.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[expect(
            clippy::enum_variant_names,
            reason = "each variant names its diagnostic, as `DiagnosticKind`'s do"
        )]
        pub(crate) enum Code {
            $($variant,)*
        }

        impl Code {
            pub(crate) const ALL: &[Code] = &[$(Code::$variant,)*];

            /// The code as written: `E0001`.
            pub(crate) fn as_str(self) -> &'static str {
                match self {
                    $(Code::$variant => $code,)*
                }
            }

            /// The explanation, `docs/diagnostics/<code>.md`.
            pub(crate) fn explanation(self) -> &'static str {
                match self {
                    $(Code::$variant => include_str!(
                        concat!("../../../../docs/diagnostics/", $code, ".md")
                    ),)*
                }
            }
        }

        impl DiagnosticKind {
            /// The code of the row that names the variant.
            pub(crate) fn code(&self) -> Code {
                match self {
                    $(
                        DiagnosticKind::$variant { .. } $(| DiagnosticKind::$alias { .. })*
                            => Code::$variant,
                    )*
                }
            }
        }
    };
}

codes! {
    AssignToImmutable = "E0001",
    UnreachableCode = "W0001",
    NeverReassigned = "W0002",
    TodoLeft = "W0003",
    UnusedValue = "W0004",
    LeadingBinaryMinus = "E0002",
    UnclosedBracket = "E0003",
    BraceOnNewLine = "E0004",
    BracketOnNewLine = "E0005",
    ElseOnNewLine = "E0006",
    UnexpectedClosingBracket = "E0007",
    DigitSeparator = "E0008",
    IntegerTooLarge = "E0009",
    UnexpectedCharacter = "E0010",
    Expected = "E0011",
    Var = "E0013",
    OptionalNests = "E0014",
    FieldComma = "E0015",
    FieldLet = "E0016",
    MethodNeedsSelf = "E0017",
    MutSelf = "E0018",
    MutParameter = "E0019",
    SelfNotFirst = "E0020",
    AssignToExpression = "E0021",
    AssignThroughSafeAccess = "E0022",
    RangeOutsideFor = "E0023",
    EqualityChain = "E0024",
    ComparisonDirection = "E0025",
    BuiltInName = "E0026",
    NeverConstructed = "E0027",
    NoMain = "E0028",
    MainSignature = "E0029",
    UnknownType = "E0030",
    BodyHasNoValue = "E0031",
    Mismatch | PatternMismatch = "E0032",
    BindingNotFound = "E0033",
    DefinedTwice = "E0034",
    ForNeedsArray = "E0035",
    ReturnNeedsValue = "E0036",
    UnexpectedReturnValue = "E0037",
    JumpOutsideLoop = "E0038",
    AlreadyDeclared = "E0039",
    IntegerDoesNotFit = "E0040",
    SelfOutsideMethod = "E0041",
    NegatedU8 = "E0042",
    NoValue = "E0043",
    NeverNone = "E0044",
    IsTestsNone = "E0045",
    IsNeedsObjects = "E0046",
    CannotCompare = "E0047",
    CoalesceNotOptional = "E0048",
    SafeAccessNotOptional = "E0049",
    CannotInfer = "E0050",
    UnwrapNeedsOptional = "E0051",
    FunctionNotFound = "E0052",
    ArgumentCount = "E0053",
    AlreadyConverted = "E0054",
    LabeledArgument = "E0055",
    MissingField = "E0056",
    MethodNotField = "E0057",
    FieldNotMethod = "E0058",
    PrintArgument = "E0059",
    PrintArray = "E0060",
    PrintNever = "E0061",
    MayBeNone = "E0062",
    NoMember = "E0063",
    TooManyArguments = "E0064",
    ArgumentNeedsLabel = "E0065",
    FieldOutOfOrder = "E0066",
    SameObject = "E0067",
    CannotIndex = "E0068",
    SumNeedsNumbers = "E0069",
    MethodAsFunction = "E0070",
    NestingTooDeep = "E0071",
    InvalidUtf8 = "E0072",
    AssignToSelf = "E0073",
    AssignToFunction = "E0074",
    BasePrefixUpperCase = "E0075",
    BasePrefixWithoutDigits = "E0076",
    DigitOutsideBase = "E0077",
    MutOnUnderscore = "E0078",
    UnderscoreValue = "E0079",
    UnderscoreName = "E0080",
    BlockAsStatement = "E0081",
    JumpInLoopHeader = "E0082",
    ReservedWord = "E0083",
    ConstInFunction = "E0084",
    ConstantType = "E0085",
    ConstantNotKnown = "E0086",
    ConstantOrder = "E0087",
    ConstantPanics = "E0088",
    UnknownEscape = "E0089",
    MalformedUnicodeEscape = "E0090",
    NotScalarValue = "E0091",
    NotOneCodePoint = "E0092",
    LiteralNotClosed = "E0093",
    ArithmeticOnChar = "E0094",
    BraceInString = "E0095",
    EmptyInterpolation = "E0096",
    CommentInInterpolation = "E0097",
    CannotInterpolate = "E0098",
    TodoForm = "E0099",
    ConversionInConstant = "E0100",
    FloatNeedsDigits = "E0101",
    ExponentNeedsDigits = "E0102",
    FloatOutOfRange = "E0103",
    NotExactlyF64 = "E0104",
    TypeAsValue = "E0105",
    MathIdiom = "E0106",
    PowerOperator = "E0107",
    MalformedFormatSpec = "E0108",
    UnsupportedFormatSpec = "E0109",
    FormatSpecLimit = "E0110",
    PrecisionNeedsF64 = "E0111",
    VariantAfterMethod = "E0112",
    VariantCase = "E0113",
    MixedLabels = "E0114",
    EmptyEnum = "E0115",
    EnumNamedLocal = "E0116",
    BareVariant = "E0117",
    UnknownVariant = "E0118",
    VariantForm = "E0119",
    MemberKind = "E0120",
    EnumField = "E0121",
    MatchIdiom = "E0122",
    PatternForm = "E0123",
    CannotMatch = "E0124",
    NotVariantHere = "E0125",
    ConstantInPattern = "E0126",
    OnlyNamePosition = "E0127",
    SubpatternCount = "E0128",
    BoundTwice = "E0129",
    NonExhaustive = "E0130",
    AlwaysMatches = "E0131",
    ForeignRange = "E0132",
    ArmSplit = "E0133",
    InterpolatedPattern = "E0134",
    BadRange = "E0135",
    AlternativeBindings = "E0136",
    NanComparison = "W0005",
    UnreachableArm = "W0006",
    RedundantSemicolon = "W0007",
}

/// Codes the compiler no longer reports, and their explanations (`docs/diagnostics/README.md`,
/// "Numbers"). `sisu explain` still prints them.
pub(crate) const RETIRED: &[(&str, &str)] = &[(
    "E0012",
    include_str!("../../../../docs/diagnostics/E0012.md"),
)];

impl Code {
    /// The code `text` names, in either case.
    pub(crate) fn parse(text: &str) -> Option<Code> {
        Code::ALL
            .iter()
            .copied()
            .find(|code| code.as_str().eq_ignore_ascii_case(text))
    }

    /// An `E` code is an error, a `W` code a warning.
    pub(crate) fn severity(self) -> Severity {
        if self.as_str().starts_with('E') {
            Severity::Error
        } else {
            Severity::Warning
        }
    }
}

/// What went wrong, and where: the facts each diagnostic code is built from.
pub(crate) enum DiagnosticKind {
    /// An assignment at `at` to `name`, or to an element of it, which cannot change.
    AssignToImmutable {
        name: String,
        at: Span,
        element: bool,
        cause: Immutable,
    },
    /// A statement at `at` after one that never finishes.
    UnreachableCode { at: Span },
    /// The `let mut` binding `name`, at `at`, which nothing reassigns; `keyword` is its `mut`.
    NeverReassigned {
        name: String,
        at: Span,
        keyword: Span,
    },
    /// A binary `-` at `at` that starts a line.
    LeadingBinaryMinus { at: Span },
    /// The `bracket` at `at`, which nothing closes.
    UnclosedBracket { bracket: char, at: Span },
    /// A `{` at `at` that starts a line; `join`, when the line above can take it, moves it up.
    BraceOnNewLine { at: Span, join: Option<Join> },
    /// A `[` at `at` that starts a line inside brackets, after an expression.
    BracketOnNewLine { at: Span },
    /// An `else` at `at` that starts a line; `join`, when a `}` ends the line above, moves it up.
    ElseOnNewLine { at: Span, join: Option<Join> },
    /// A closing `bracket` at `at` with no match; `opened` is the bracket that is open instead.
    UnexpectedClosingBracket {
        bracket: char,
        at: Span,
        opened: Option<(char, Span)>,
    },
    /// A `_` at `at` that is not between digits.
    DigitSeparator { at: Span },
    /// An integer literal at `at` that does not fit `i64`.
    IntegerTooLarge { at: Span },
    /// The `.` at `at` of a float with no digit on one side of it: `1.` or `.5`. `fix` is where
    /// the missing `0` goes, after the `.` in the first form and before it in the second.
    FloatNeedsDigits { at: Span, fix: Span },
    /// The exponent at `at`, from its `e` or `E` through its sign, with no digit after it.
    ExponentNeedsDigits { at: Span },
    /// A float literal at `at` that rounds to infinity in `f64`.
    FloatOutOfRange { at: Span },
    /// The base prefix at `at` (`0` and `prefix`, an upper case `X`, `B` or `O`). `safe` is
    /// false when the lower case literal may still be rejected, as a bare 2^63 is.
    BasePrefixUpperCase { prefix: char, safe: bool, at: Span },
    /// The base prefix at `at` (`0` and `prefix`) with no digits after it.
    BasePrefixWithoutDigits { prefix: char, at: Span },
    /// The `digit`, at `at`, in a literal of base `radix` (2, 8 or 16), which is not a digit there.
    DigitOutsideBase { digit: char, radix: u32, at: Span },
    /// The character `c` at `at`, which Sisu does not use.
    UnexpectedCharacter { c: char, at: Span },
    /// The token `found` at `at`, where `what` should stand.
    Expected {
        what: String,
        found: String,
        at: Span,
    },
    /// A `var` at `at`, on a field or on a binding.
    Var { at: Span, field: bool },
    /// The type `ty`, then `?` again, from `at`.
    OptionalNests { ty: String, at: Span },
    /// A `,` at `at` after a `member`. `replacement` is what takes the comma: nothing when it ends
    /// its line or comes before `}`, and `;` otherwise.
    FieldComma {
        at: Span,
        member: Member,
        replacement: String,
    },
    /// A `let` at `at` in front of a field.
    FieldLet { at: Span, rest: LetField },
    /// A method named at `at` with no `self`.
    MethodNeedsSelf { at: Span },
    /// A `mut` at `keyword` in front of the `self` at `receiver`, in a method of `owner`. In a
    /// class, removing it is `safe` when `self` is the whole parameter; in an enum it never is.
    MutSelf {
        keyword: Span,
        receiver: Span,
        safe: bool,
        owner: Owner,
    },
    /// A `mut` at `at` in front of the parameter `name`.
    MutParameter { name: String, at: Span },
    /// A `self` at `at` that is not a method's first parameter.
    SelfNotFirst { at: Span },
    /// An assignment at `at` to something that is not a place.
    AssignToExpression { at: Span },
    /// An assignment at `at` through a `?.`.
    AssignThroughSafeAccess { at: Span },
    /// A range at `at` outside a `for`.
    RangeOutsideFor { at: Span },
    /// A second `==`, `!=` or `is` at `at` in one chain.
    EqualityChain { at: Span },
    /// A comparison at `at` that turns a chain around.
    ComparisonDirection { at: Span },
    /// The built-in `name`, at `at`, taken by a class, function, constant, enum, variant or
    /// binding.
    BuiltInName {
        name: String,
        at: Span,
        kind: BuiltIn,
    },
    /// The class or enum `name`, which no value can have, reported at `at` for `cause`.
    NeverConstructed {
        name: String,
        at: Span,
        cause: Unbuilt,
    },
    /// A program with no `main`.
    NoMain,
    /// A `main` at `at` that takes anything but nothing or one `[string]`, or returns a value.
    MainSignature { at: Span },
    /// The type `name`, at `at`, which is not built in and names no class.
    UnknownType {
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// The function `function`, whose body, ending at `at`, has no value of type `want`.
    BodyHasNoValue {
        function: String,
        want: String,
        at: Span,
    },
    /// A value of type `got`, at `at`, where one of the types `want` is expected; `fix` is the
    /// rewrite that would fit. `source` is the place the expectation came from, if the report
    /// points at it. `conversion` is whether the value is a conversion that can fail, whose
    /// optional stands where its payload is expected.
    Mismatch {
        want: Vec<String>,
        got: String,
        at: Span,
        source: Option<MismatchSource>,
        fix: Option<MismatchFix>,
        conversion: bool,
    },
    /// The binding `name`, at `at`, which no scope holds.
    BindingNotFound {
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// A `kind` definition named `name`, at `at`, after an earlier definition of that name,
    /// `first`.
    DefinedTwice {
        kind: Definition,
        name: String,
        at: Span,
        first: Earlier,
    },
    /// A `for` over the value at `at`, of type `found`, which is no array or range.
    ForNeedsArray { found: String, at: Span },
    /// A `return` at `at` with no value, in a function that returns a `want`.
    ReturnNeedsValue { want: String, at: Span },
    /// A `return` with the value at `at`, in a function that returns none.
    UnexpectedReturnValue { at: Span },
    /// A `break` or `continue` (`keyword`) at `at`, outside a loop.
    JumpOutsideLoop { keyword: &'static str, at: Span },
    /// The parameter `name`, at `at`, which an earlier parameter at `first` already has.
    AlreadyDeclared { name: String, at: Span, first: Span },
    /// The integer literal `text` as written, at `at`, which does not fit the `u8` its position
    /// expects.
    IntegerDoesNotFit { text: String, at: Span },
    /// The integer literal `text` as written, at `at`, which the `f64` its position expects
    /// does not hold exactly; `nearest` is the text of the `f64` it rounds to.
    NotExactlyF64 {
        text: String,
        nearest: String,
        at: Span,
    },
    /// A `self` at `at`, outside a method.
    SelfOutsideMethod { at: Span },
    /// A unary `-` at `at` on a `u8`.
    NegatedU8 { at: Span },
    /// An expression at `at` that has no value: it is `unit`, or, with `never`, it never
    /// finishes.
    NoValue { at: Span, never: bool },
    /// A comparison at `at` of a value that is not optional with `None`. `value` is that value's
    /// path as written, when it has one.
    NeverNone { value: Option<String>, at: Span },
    /// An `is` at `at` with `None` on one side. `tested`, when `== None` would be right, is the
    /// path of the other side as written.
    IsTestsNone { tested: Option<String>, at: Span },
    /// An `is` whose operand at `at`, of type `found`, is no object.
    IsNeedsObjects { found: String, at: Span },
    /// A comparison at `at` of a `left` with a `right`.
    CannotCompare {
        left: String,
        right: String,
        at: Span,
    },
    /// The left side of a `??`, at `at`, which is not optional.
    CoalesceNotOptional { at: Span },
    /// The receiver at `at` of a `?.`, which is not optional. `value` is its path as written,
    /// when it has one. `access` is everything between the receiver and the member name: the
    /// `?.` and any space or comment around it.
    SafeAccessNotOptional {
        value: Option<String>,
        at: Span,
        access: Span,
    },
    /// A `None` or `[]` (`literal`) at `at` whose type nothing fixes.
    CannotInfer { literal: Literal, at: Span },
    /// The value at `at`, of type `found`, that `keyword` unwraps, and which is not optional.
    UnwrapNeedsOptional {
        keyword: Unwrap,
        found: String,
        at: Span,
        /// Whether `found` is a type `==` compares: `i64`, `u8`, `bool`, `string` or `char`.
        compare: bool,
    },
    /// A call at `at` of `name`, which names no function.
    FunctionNotFound {
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// A call at `at` of `name`, which takes `want` arguments, with `found`.
    ArgumentCount {
        name: String,
        want: usize,
        found: usize,
        at: Span,
    },
    /// A conversion to `ty` of the value at `at`, which already has that type. `value` is its
    /// path as written, when it has one, `conversion` is the whole call, with any parentheses
    /// around it, and `name` the type's name that starts the call. `grouped` is set when the
    /// value is more than one term, so removing the conversion keeps its parentheses.
    /// `labeled` is set when the argument has a label, which the removal would delete along with
    /// its own error, so there is no fix. `commented` is set when the removal would delete a
    /// comment, which makes it unsafe.
    AlreadyConverted {
        value: Option<String>,
        ty: String,
        at: Span,
        conversion: Span,
        name: Span,
        grouped: bool,
        labeled: bool,
        commented: bool,
    },
    /// A label at `at` on an argument of a call that is not a constructor. A call of a variant
    /// that declares no labels is reported once, at its first label: `variant` holds them all.
    LabeledArgument {
        at: Span,
        variant: Option<VariantLabels>,
    },
    /// A constructor at `at` without the field `field`; `fields` are its class's, in order.
    MissingField {
        field: String,
        fields: Vec<String>,
        at: Span,
    },
    /// The method `name` of `owner`, at `at`, read as a field. `call` is whether adding `()`
    /// is safe to fix it, when it can be added.
    MethodNotField {
        name: String,
        owner: String,
        at: Span,
        call: Option<bool>,
    },
    /// The field `name` of `owner`, at `at`, called as a method.
    FieldNotMethod {
        name: String,
        owner: String,
        at: Span,
    },
    /// A call of `function` (`print` or `eprint`) at `at` with no argument, several, or one it
    /// cannot print. `class` is the class or enum of an object or enum argument, and `at` is
    /// then that argument.
    PrintArgument {
        function: &'static str,
        at: Span,
        class: Option<String>,
    },
    /// The argument at `at` of a call of `function`, of the array type `ty` or an optional of one.
    PrintArray {
        function: &'static str,
        ty: String,
        optional: bool,
        at: Span,
    },
    /// The argument at `at` of a call of `function`, which never finishes.
    PrintNever { function: &'static str, at: Span },
    /// The optional at `at` that a member, an index or a `for` uses as a value. `value` is its
    /// path as written, when it has one. `access` is the `.` before the member, and any space or
    /// comment around it, when a `?.` can take its place.
    MayBeNone {
        value: Option<String>,
        at: Span,
        access: Option<Span>,
    },
    /// The `kind` member `name`, at `at`, which `owner` does not have.
    NoMember {
        owner: String,
        kind: Definition,
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// The constructor argument at `at`, after the last of the `fields` fields of `class`.
    TooManyArguments {
        class: String,
        fields: usize,
        at: Span,
    },
    /// The constructor argument at `at`, without a label; `field` is the field it sets.
    ArgumentNeedsLabel { field: String, at: Span },
    /// The label `found`, at `at`, where the constructor names `field`; `fields` are the
    /// class's, in order.
    FieldOutOfOrder {
        field: String,
        found: String,
        fields: Vec<String>,
        at: Span,
    },
    /// The value at `at` of an array fill, an object of `class` or an optional of one.
    SameObject { class: String, at: Span },
    /// An index into the value at `at`, of type `ty`, which is no array.
    CannotIndex { ty: String, at: Span },
    /// A `sum` at `at` of an array of type `ty`, which is neither `[i64]` nor `[f64]`.
    SumNeedsNumbers { ty: String, at: Span },
    /// A call at `at` of the method `name` of an array or a number as a function, with the
    /// receiver as its first argument. `next` is the next argument, if any. `wrap` is whether
    /// `receiver` needs parentheses to take a `.name(...)`, and `safe` whether the method call
    /// checks.
    MethodAsFunction {
        name: String,
        at: Span,
        receiver: Span,
        next: Option<Span>,
        wrap: bool,
        safe: bool,
    },
    /// The token at `at`, where the tree passes `limit` levels.
    NestingTooDeep { at: Span, limit: usize },
    /// The first byte at `at` that is not UTF-8, which the text shown in its place replaces.
    InvalidUtf8 { at: Span },
    /// An assignment to the `self` at `at`.
    AssignToSelf { at: Span },
    /// An assignment to `name`, at `at`, which is a function.
    AssignToFunction { name: String, at: Span },
    /// A `let` that binds the `_` at `at` and has the `mut` at `keyword`.
    MutOnUnderscore { at: Span, keyword: Span },
    /// The `_` at `at` where a value belongs. `bind` is whether it starts the plain assignment
    /// `_ = e`, which `let ` in front of it turns into a binding.
    UnderscoreValue { at: Span, bind: bool },
    /// The `_` at `at`, where a name belongs.
    UnderscoreName { at: Span },
    /// The `{` at `at`, which starts a statement.
    BlockAsStatement { at: Span },
    /// A `break` or `continue` (`keyword`) at `at`, in the header of the loop whose keyword is at
    /// `loop_at`.
    JumpInLoopHeader {
        keyword: &'static str,
        at: Span,
        loop_at: Span,
    },
    /// The word `word` at `at`, which a later version of Sisu gives a meaning.
    ReservedWord { word: &'static str, at: Span },
    /// The `const` at `at`, which starts a statement.
    ConstInFunction { at: Span },
    /// The type at `at` of a constant, which is none a constant can have.
    ConstantType { at: Span },
    /// The part at `at` of a constant's value, which is not known at compile time; `interpolation`
    /// when it is an interpolated string.
    ConstantNotKnown { at: Span, interpolation: bool },
    /// The use at `at`, in the value of the constant `user`, of the constant `name` declared at
    /// `declared`: `itself` when `name` is `user`, else `name` is declared below `user`.
    ConstantOrder {
        name: String,
        user: String,
        at: Span,
        declared: Span,
        itself: bool,
    },
    /// The operation at `at` in a constant's value, which would `panic` at run time.
    ConstantPanics { panic: Panic, at: Span },
    /// The escape `\` and `found`, at `at` in a `literal`, which is none of the escapes.
    UnknownEscape {
        found: char,
        at: Span,
        literal: LiteralKind,
    },
    /// The `\u` escape at `at`, as far as it reaches, which is not 1 to 6 hex digits in braces.
    MalformedUnicodeEscape { at: Span },
    /// The `\u{...}` escape `escape`, as written, at `at`, whose value is not a Unicode scalar
    /// value.
    NotScalarValue { escape: String, at: Span },
    /// The `char` literal at `at`, which holds no code point or more than one; `string` when its
    /// text holds no `"`, `{` or `}`, which a string literal would read differently.
    NotOneCodePoint { at: Span, string: bool },
    /// The literal of `kind` that starts at `at` and stops at a line break or the end of the file;
    /// `interpolation` when an interpolation's `{` is open there; `end_of_file` when the end of
    /// the file, not a line break, stopped it.
    LiteralNotClosed {
        kind: LiteralKind,
        at: Span,
        interpolation: bool,
        end_of_file: bool,
    },
    /// Arithmetic on the `char` at `at`.
    ArithmeticOnChar { at: Span },
    /// The `}` at `at` in the text of a string literal, which no interpolation opened.
    BraceInString { at: Span },
    /// The interpolation at `at`, its braces included, which holds only spaces.
    EmptyInterpolation { at: Span },
    /// The `//` at `at`, inside an interpolation.
    CommentInInterpolation { at: Span },
    /// The interpolated value at `at`, of type `ty`, which interpolation cannot write as text;
    /// `help` is the advice for its kind of type, if there is one.
    CannotInterpolate {
        ty: String,
        help: Option<InterpolateHelp>,
        at: Span,
    },
    /// The `todo` at `at`, which panics when it runs.
    TodoLeft { at: Span },
    /// A `todo` written as Rust's `todo!` or as `todo()`: `at` is the first extra token, and
    /// `delete` is the text whose removal leaves bare `todo` or `todo(message)`; unless `safe`, the
    /// removal joins `todo` to the word after it.
    TodoForm { at: Span, delete: Span, safe: bool },
    /// The conversion to `to` at `at`, in a constant's value, which can fail; `fix` is the
    /// literal that replaces it, when the value is known and a literal shows it plainly.
    ConversionInConstant {
        to: String,
        at: Span,
        fix: Option<TypedLiteral>,
    },
    /// The expression statement, or branch or body tail, at `at`, whose value of type `ty` nothing
    /// uses. `arm` when it is a `match` arm's whole body, which takes no statement outside a block.
    /// `sites` are the values the fix drops, `at` among them, each with its own `arm`: those of
    /// the whole statement, since a branch with no value beside one with a value is an error.
    /// Empty when the statement has too many values for one fix to list; then there is no fix.
    UnusedValue {
        ty: String,
        at: Span,
        arm: bool,
        sites: Vec<(Span, bool)>,
    },
    /// The built-in type or class `name`, at `at`, read as a value.
    TypeAsValue { name: String, at: Span },
    /// The member of `math`, `Math` or `f64`, written at `written`, which is not how Sisu writes
    /// math: a method or constant of another language, or `f64.max(a, b)` for `a.max(b)`; `fix`
    /// is the form Sisu uses, when it surely checks.
    MathIdiom { written: Span, fix: Option<MathFix> },
    /// The `**` at `at`, which Sisu has no operator for; `fix` is the `pow` call that may mean it.
    PowerOperator { at: Span, fix: Option<PowerFix> },
    /// The format spec at `at`, its `:` included, which the grammar `[<^>][width][.precision]`
    /// does not hold: it is empty, an alignment alone, or has a `.` without digits.
    MalformedFormatSpec { at: Span },
    /// The format spec at `at`, its `:` included, which asks for more than alignment, width and
    /// precision: a leading `0`, a sign, a type, a fill, a space or a nested `{`.
    UnsupportedFormatSpec { at: Span },
    /// The format spec at `at`, its `:` included, whose width, when `width`, or else precision is
    /// above its limit.
    FormatSpecLimit { at: Span, width: bool },
    /// The precision of the spec at `at` on a part of type `ty`, which is not `f64`; `fix` is the
    /// part, when it is an `i64` that `f64(...)` converts.
    PrecisionNeedsF64 {
        ty: String,
        at: Span,
        fix: Option<Span>,
    },
    /// The variant at `at`, which follows the method whose `fn` is at `first_method`.
    VariantAfterMethod { at: Span, first_method: Span },
    /// The variant `name`, at `at`, which starts with no upper-case letter.
    VariantCase { name: String, at: Span },
    /// A payload of the variant `variant` that labels some fields and not others; `at` is the
    /// first field labeled differently from the first.
    MixedLabels { variant: String, at: Span },
    /// The enum `name`, at `at`, which declares no variant.
    EmptyEnum { name: String, at: Span },
    /// A local, parameter or binding at `at` named `name`, which names an enum.
    EnumNamedLocal { name: String, at: Span },
    /// The variant `name`, at `at`, written without its enum where no item or binding takes the
    /// name; `enums` are the enums that have such a variant, in declaration order.
    BareVariant {
        name: String,
        enums: Vec<String>,
        at: Span,
    },
    /// The variant `name`, at `at`, which the enum `enum_name` does not have; `suggestion` is its
    /// closest variant.
    UnknownVariant {
        enum_name: String,
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// The variant `path` (`State.Idle`) built the wrong way. With `payload` it has one and is not
    /// called, and `at` is the variant's name. Without, it holds nothing and is called, and `at`
    /// is the parentheses with what they hold; `held` says whether they hold anything.
    VariantForm {
        path: String,
        payload: bool,
        held: bool,
        at: Span,
    },
    /// The member `name`, at `at`, of the enum `enum_name`, used as the other kind of member: a
    /// `method` named through the enum, or else a variant called on a value.
    MemberKind {
        name: String,
        enum_name: String,
        method: bool,
        at: Span,
    },
    /// The field at `at`, read or assigned on a value of the enum `enum_name`.
    EnumField { enum_name: String, at: Span },
    /// An idiom of another language for an enum or `match`, at `at`.
    MatchIdiom { form: MatchIdiomForm, at: Span },
    /// A pattern written another language's way, from `at`.
    PatternForm { form: ForeignPattern, at: Span },
    /// The scrutinee at `at`, of type `ty`, which `match` cannot take.
    CannotMatch { ty: String, at: Span },
    /// The upper-case name `name`, at `at`, in a pattern where no enum is expected.
    NotVariantHere { name: String, at: Span },
    /// The constant `name`, at `at`, written as a pattern. `guard` is the guard the help shows:
    /// a name and the guard that compares it with the constant, `n if n == LIMIT`, or in words
    /// in an arm with `|`; only in a `match` arm, and not for a range bound, where `n == LIMIT`
    /// would change what the range means.
    ConstantInPattern {
        name: String,
        at: Span,
        guard: Option<GuardHelp>,
    },
    /// The pattern at `at`, neither a name nor `_`, at a position of type `ty`, which takes only
    /// those.
    OnlyNamePosition { ty: String, at: Span },
    /// The pattern at `at` for `variant`, which has `fields` fields, with `found` sub-patterns.
    SubpatternCount {
        variant: String,
        fields: usize,
        found: usize,
        at: Span,
    },
    /// The name `name`, at `at`, bound again in one pattern; `first` is its first binding.
    /// `guard` is the guard the help shows, in a `match` arm.
    BoundTwice {
        name: String,
        at: Span,
        first: Span,
        guard: Option<GuardHelp>,
    },
    /// The pattern at `at`, `found` as written (`Shape.Circle`, `None`), at a position of type
    /// `want`.
    PatternMismatch {
        want: String,
        found: String,
        at: Span,
    },
    /// The `match` whose keyword is at `at`, which does not cover `missing`. `partly` is the
    /// first arm with `missing`'s variant at the top: its span, its pattern and its payload.
    /// `guarded` is whether a guarded arm may not match it, and `gap` whether it is a gap in an
    /// integer, `char` or `string` type, which only `_ =>` closes for good.
    /// The fix writes a `_ =>` arm, indented by `indent`, in place of `insert`: an empty span,
    /// or the space between the last arm and a `}` on its line. `close` is the indentation of
    /// that `}`, which the fix moves to a line of its own.
    NonExhaustive {
        missing: String,
        partly: Option<(Span, String, String)>,
        guarded: bool,
        gap: bool,
        at: Span,
        insert: Span,
        indent: String,
        close: Option<String>,
    },
    /// The arm whose pattern is at `at`, or the alternative at `at` when `alternative`, which no
    /// value reaches; `covered_by` is the arm that takes all its values, when one does.
    /// `remove` is the arm's line, or, when the `match`'s `}` ends that line, from the line
    /// break before the arm through its body; for an alternative, the alternative and the `|`
    /// that joins it to the one before it, or after it when it is first.
    UnreachableArm {
        at: Span,
        alternative: bool,
        covered_by: Option<Span>,
        remove: Span,
    },
    /// A `;` at `at` that separates nothing. `space` when it touches a token on each side, so
    /// that removing it would join them.
    RedundantSemicolon { at: Span, space: bool },
    /// The pattern at `at` of `keyword`, which every value of its enum matches.
    AlwaysMatches { keyword: Unwrap, at: Span },
    /// A range written another language's way, with its operator at `at`.
    ForeignRange { form: ForeignRange, at: Span },
    /// A line of an arm list, from `at`, its first token, that starts with `|`, `=>` or a
    /// guard's `if`, and so continues the arm above. `join` is the space from the end of the
    /// line above to `at`, which the fix replaces with one space; `None` when a comment lies in
    /// it.
    ArmSplit { at: Span, join: Option<Span> },
    /// The interpolation at `at` in a string pattern.
    InterpolatedPattern { at: Span },
    /// The range pattern at `at`, which `form` says is wrong.
    BadRange { form: BadRange, at: Span },
    /// The alternative at `at`, which does not bind `name`, though another alternative of the
    /// same `|` does.
    AlternativeBindings { name: String, at: Span },
    /// The comparison at `at`, `!=` when `negated`, one side of which is `f64.nan`, which no
    /// number equals; `fix` is its `is_nan` test, when the other side is an `f64`.
    NanComparison {
        at: Span,
        negated: bool,
        fix: Option<NanFix>,
    },
}

/// The other side of a comparison with `f64.nan`, at `other`, and whether it needs parentheses to
/// take `.is_nan()`.
#[derive(Clone, Copy)]
pub(crate) struct NanFix {
    pub(crate) other: Span,
    pub(crate) wrap: bool,
}

/// What replaces a `math` idiom written at `written`.
pub(crate) enum MathFix {
    /// The constant `f64.<name>`.
    Constant { name: &'static str },
    /// The method call `receiver.<name>(next...)` for `written(receiver, next...)`. `wrap` is
    /// whether `receiver` needs parentheses to take the call, and `form` is the call as it
    /// reads.
    Method {
        name: &'static str,
        form: String,
        receiver: Span,
        next: Option<Span>,
        wrap: bool,
    },
}

/// The operands of a `**`, which `left.pow(right)` replaces.
#[derive(Clone, Copy)]
pub(crate) struct PowerFix {
    pub(crate) left: Span,
    pub(crate) right: Span,
}

/// A literal that replaces the conversion `call`, the whole value of a constant: `literal` is
/// its text, and `annotate`, when the constant declares no type, inserts `: <type>` at an offset.
pub(crate) struct TypedLiteral {
    pub(crate) call: Span,
    pub(crate) literal: String,
    pub(crate) annotate: Option<(usize, String)>,
}

/// The variant `path` (`Expr.Num`) whose payload declares no labels, and each `label: ` of a call
/// of it, which its fix deletes.
pub(crate) struct VariantLabels {
    pub(crate) path: String,
    pub(crate) labels: Vec<Span>,
}

/// What a literal is the literal of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiteralKind {
    Char,
    String,
}

/// A run-time panic that folding a constant meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panic {
    Overflow,
    DivisionByZero,
}

impl Panic {
    /// The run-time panic's message, which codegen also uses.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Panic::Overflow => "integer overflow",
            Panic::DivisionByZero => "division by zero",
        }
    }
}

/// A literal that takes its type from its position.
#[derive(Clone, Copy)]
pub(crate) enum Literal {
    None,
    EmptyArray,
}

/// The advice for a value that interpolation cannot write as text.
#[derive(Clone, Copy)]
pub(crate) enum InterpolateHelp {
    /// An array, or an optional of one.
    ForLoop,
    /// Any other optional.
    Unwrap,
}

/// The statement or expression that unwraps an optional.
#[derive(Clone, Copy)]
pub(crate) enum Unwrap {
    IfLet,
    WhileLet,
}

impl Unwrap {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Unwrap::IfLet => "if let",
            Unwrap::WhileLet => "while let",
        }
    }
}

/// What a name is defined as.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Definition {
    Function,
    Class,
    Field,
    Method,
    Binding,
    Type,
    Constant,
    Enum,
    Variant,
}

impl Definition {
    /// The indefinite article before `word`.
    fn article(self) -> &'static str {
        match self {
            Definition::Enum => "an",
            _ => "a",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Definition::Function => "function",
            Definition::Class => "class",
            Definition::Field => "field",
            Definition::Method => "method",
            Definition::Binding => "binding",
            Definition::Type => "type",
            Definition::Constant => "constant",
            Definition::Enum => "enum",
            Definition::Variant => "variant",
        }
    }
}

/// Where the earlier definition of a name is.
#[derive(Clone)]
pub(crate) enum Earlier {
    /// In the source, at `span`, as a `kind`; `owner` is the class of a field or method.
    At {
        span: Span,
        kind: Definition,
        owner: Option<String>,
    },
    /// In the language: a prelude function, such as `print`.
    BuiltIn,
}

/// Converting the expression at `operand` to the type `to`: `u8`, `i64` or `char`. `u8` and
/// `char` convert into each other `through` `i64`.
#[derive(Clone, Copy)]
pub(crate) struct Conversion {
    pub(crate) to: &'static str,
    pub(crate) through: Option<&'static str>,
    pub(crate) operand: Span,
}

/// Where a mismatch's expected type came from.
pub(crate) enum MismatchSource {
    /// The `string` operand of a `+` that expects a `string` of the other.
    Concat(Span),
    /// The earlier alternative's binding of `name`, at `at`, which gave it the type wanted.
    Binding { name: String, at: Span },
}

/// A rewrite that would give a mismatched value the type expected.
#[derive(Clone, Copy)]
pub(crate) enum MismatchFix {
    Convert(Conversion),
    /// A `string` `+` a value that is not one, as one interpolated literal of both operands.
    Interpolate {
        lhs: Operand,
        rhs: Operand,
    },
    /// A `string(arg)` call, over `call`, of a value that interpolation prints, as the literal
    /// `"{arg}"`.
    InterpolateCall {
        call: Span,
        arg: Span,
    },
    /// A value that interpolation prints, on one line, as the literal `"{value}"`.
    WrapInString(Span),
}

/// An operand of a `+` that interpolation joins: where it is written, and whether it is a
/// string literal, which loses its quote on the joined side instead of being wrapped.
#[derive(Clone, Copy)]
pub(crate) struct Operand {
    pub(crate) span: Span,
    pub(crate) literal: bool,
}

/// Moving a token up to the code before it: the `gap` between them becomes a space.
#[derive(Clone, Copy)]
pub(crate) struct Join {
    pub(crate) gap: Span,
    /// Whether the move is always right, not only likely.
    pub(crate) safe: bool,
}

/// What a comma follows, in a list that puts each on its own line.
#[derive(Clone, Copy)]
pub(crate) enum Member {
    Field,
    Variant,
    Arm,
}

impl std::fmt::Display for Member {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Member::Field => "field",
            Member::Variant => "variant",
            Member::Arm => "arm",
        })
    }
}

/// The idiom of another language that a `MatchIdiom` reports, at the diagnostic's span.
pub(crate) enum MatchIdiomForm {
    /// `switch` for `match`.
    Switch,
    /// `case` starting an arm; `remove` is `case ` and, for Swift's `case let .p`, `let ` and the
    /// `.`.
    Case { remove: Vec<Span> },
    /// `default:` starting an arm.
    Default,
    /// `else` starting an arm, before its `=>`.
    Else,
    /// `->` after an arm's pattern.
    Arrow,
    /// `class` after `enum`, in the enum `name`; `remove` is it and the spaces after it.
    EnumClass { name: String, remove: Span },
    /// `::` between `name` and `member`.
    Path { name: String, member: String },
}

/// A pattern written another language's way. `form` is how Sisu writes it: `Rect(x, y)`.
pub(crate) enum ForeignPattern {
    /// Labels on the sub-patterns; `labels` are each `label: ` the fix deletes.
    Labels { form: String, labels: Vec<Span> },
    /// Rust's braces, `Rect { w, h }`; `replace` is what the fix replaces with `(w, h)`, from the
    /// end of the name through the `}`.
    Braces {
        form: String,
        replace: Span,
        fields: String,
    },
}

/// A range written another language's way. `start` and `end` are its bounds as written; `end` is
/// `None` when it did not parse, and the message then calls it `end`.
pub(crate) enum ForeignRange {
    /// `start..<end`; `remove` is the `<`, unless removing it leaves a range the parser rejects.
    Exclusive {
        start: String,
        end: Option<String>,
        remove: Option<Span>,
    },
    /// `start...end`.
    Ellipsis { start: String, end: Option<String> },
}

/// The guard the help of `BoundTwice` or `ConstantInPattern` suggests.
pub(crate) enum GuardHelp {
    /// Written out: for `BoundTwice` the arm's pattern with the second binding renamed and the
    /// guard that compares the two, for `ConstantInPattern` a name and the guard.
    Rewritten(String),
    /// In words: the arm holds an `|`, whose other alternatives would not bind the name the
    /// guard reads.
    InWords,
}

/// What is wrong with a range pattern.
pub(crate) enum BadRange {
    /// It stands at a position of a type with no ranges, or its bounds are of one.
    Type,
    /// It matches no value: `5..=4`, `5..5`.
    Empty,
}

/// What owns a method.
#[derive(Clone)]
pub(crate) enum Owner {
    Class,
    Enum(String),
}

/// What a built-in name is, and so why a program cannot take it.
#[derive(Clone, Copy)]
pub(crate) enum BuiltIn {
    /// The name of a built-in type.
    Type,
    /// A keyword.
    Keyword,
    /// A constructor milestone 8's prelude takes: `Some`, `Ok` or `Err`.
    Constructor,
}

/// Why no value of a class or enum can be built.
pub(crate) enum Unbuilt {
    /// A class whose field `field`, at the diagnostic's span, needs a `target`; `ty` is the
    /// field's type, where a `?` would break the cycle.
    Field {
        field: String,
        target: String,
        ty: Span,
    },
    /// An enum, and the links that lead from it back to it: each holder (`D.Q` or `K.d`) and
    /// the type it holds.
    Chain(Vec<(String, String)>),
}

/// What follows the `let` in front of a field.
pub(crate) enum LetField {
    /// A field that ends the line: deleting `remove`, from the `let` to the next token, fixes it.
    Whole { remove: Span },
    /// Anything else; `field` is `name: Type`, or a placeholder when none follows, and `mutable`
    /// whether `mut` follows the `let`.
    Other { mutable: bool, field: String },
}

/// Why a place cannot change; each `declared` is the name where it was declared.
pub(crate) enum Immutable {
    /// `redeclared` when the `let` shadows a `let mut`, in its scope or an outer one.
    Let {
        declared: Span,
        redeclared: bool,
    },
    Param {
        declared: Span,
    },
    /// Bound by `keyword`: `for`, `if let` or `while let`.
    Unwrapped {
        keyword: &'static str,
        declared: Span,
    },
    /// A field declared without `mut`.
    Field {
        declared: Span,
    },
    /// A top-level constant.
    Constant {
        declared: Span,
    },
}
impl From<DiagnosticKind> for Diagnostic {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per diagnostic code, so the table reads in one place"
    )]
    fn from(kind: DiagnosticKind) -> Diagnostic {
        let code = kind.code();
        match kind {
            DiagnosticKind::AssignToImmutable {
                name,
                at,
                element,
                cause,
            } => {
                let what = if element { "an element of " } else { "" };
                let d = Diagnostic::coded(code, at, format!("cannot assign to {what}`{name}`"));
                match cause {
                    Immutable::Let {
                        declared,
                        redeclared,
                    } => if element {
                        d
                    } else {
                        d.with_label("cannot assign twice")
                    }
                    .with_secondary(
                        declared,
                        if redeclared {
                            format!("`{name}` is redeclared here with `let`")
                        } else {
                            "declared with `let` here".to_string()
                        },
                    )
                    .with_fix(insert_mut("declare it with `let mut`", declared)),
                    Immutable::Param { declared } => d
                        .with_secondary(declared, "declared as a parameter here")
                        .with_help(format!(
                            "copy it into a variable: `let mut {name} = {name}`"
                        )),
                    Immutable::Unwrapped { keyword, declared } => {
                        d.with_secondary(declared, format!("bound by `{keyword}` here"))
                    }
                    Immutable::Field { declared } => d
                        .with_secondary(declared, "declared without `mut` here")
                        .with_fix(insert_mut("mark the field `mut`", declared)),
                    Immutable::Constant { declared } => if element {
                        d
                    } else {
                        d.with_label("cannot assign")
                    }
                    .with_secondary(declared, "declared with `const` here"),
                }
            }
            DiagnosticKind::UnreachableCode { at } => {
                Diagnostic::coded(code, at, "unreachable code")
            }
            DiagnosticKind::NeverReassigned { name, at, keyword } => {
                Diagnostic::coded(code, at, format!("`{name}` is never reassigned")).with_fix(Fix {
                    message: "declare it with `let`".to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: Span::new(keyword.start(), at.start()),
                        replacement: String::new(),
                    }],
                })
            }
            DiagnosticKind::LeadingBinaryMinus { at } => {
                Diagnostic::coded(code, at, "a line cannot start with a binary `-`").with_help(
                    "to continue the previous line, end it with `-`; to negate, write `-x`",
                )
            }
            DiagnosticKind::UnclosedBracket { bracket, at } => {
                Diagnostic::coded(code, at, format!("unclosed `{bracket}`"))
            }
            DiagnosticKind::BraceOnNewLine { at, join } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    "`{` must be on the same line as `fn`, `class`, `enum`, `if`, `else`, `match`, `while` or `for`",
                );
                match join {
                    Some(join) => d.with_fix(join_fix(
                        "move `{` to the end of the line above",
                        "if this `{` opens the block above, move it to the end of that line",
                        join,
                    )),
                    None => d,
                }
            }
            DiagnosticKind::BracketOnNewLine { at } => {
                Diagnostic::coded(code, at, "`[` cannot start a line here")
                    .with_help("add a comma, or put `[` on the line of its array")
            }
            DiagnosticKind::ElseOnNewLine { at, join } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    "`else` must be on the same line as the closing `}`",
                );
                match join {
                    Some(join) => d.with_fix(join_fix(
                        "move `else` after the `}`",
                        "if this `else` belongs to the `if` above, move it after the `}`",
                        join,
                    )),
                    None => d,
                }
            }
            DiagnosticKind::UnexpectedClosingBracket {
                bracket,
                at,
                opened,
            } => {
                let d = Diagnostic::coded(code, at, format!("unexpected `{bracket}`"));
                match opened {
                    Some((open, span)) => d.with_secondary(span, format!("`{open}` opened here")),
                    None => d,
                }
            }
            DiagnosticKind::DigitSeparator { at } => {
                Diagnostic::coded(code, at, "`_` must sit between digits")
            }
            DiagnosticKind::IntegerTooLarge { at } => {
                Diagnostic::coded(code, at, "integer literal is too large for `i64`")
            }
            DiagnosticKind::FloatNeedsDigits { at, fix } => {
                let side = if fix.start() > at.start() { "after" } else { "before" };
                Diagnostic::coded(code, at, "a float needs digits on both sides of `.`").with_fix(
                    Fix {
                        message: format!("write a `0` {side} the `.`"),
                        safe: true,
                        edits: vec![Edit {
                            span: fix,
                            replacement: "0".to_string(),
                        }],
                    },
                )
            }
            DiagnosticKind::ExponentNeedsDigits { at } => {
                Diagnostic::coded(code, at, "an exponent needs digits")
            }
            DiagnosticKind::FloatOutOfRange { at } => {
                Diagnostic::coded(code, at, "literal out of range for `f64`")
            }
            DiagnosticKind::BasePrefixUpperCase {
                prefix,
                safe,
                at,
            } => {
                Diagnostic::coded(code, at, "base prefixes are lower case").with_fix(Fix {
                    message: "write the prefix in lower case".to_string(),
                    safe,
                    edits: vec![Edit {
                        span: Span::new(at.end() - 1, at.end()),
                        replacement: prefix.to_ascii_lowercase().to_string(),
                    }],
                })
            }
            DiagnosticKind::BasePrefixWithoutDigits { prefix, at } => {
                Diagnostic::coded(code, at, format!("`0{prefix}` needs at least one digit"))
            }
            DiagnosticKind::DigitOutsideBase { digit, radix, at } => {
                let base = match radix {
                    2 => "a binary",
                    8 => "an octal",
                    _ => "a hexadecimal",
                };
                Diagnostic::coded(code, at, format!("`{digit}` is not {base} digit"))
            }
            DiagnosticKind::UnexpectedCharacter { c, at } => {
                let d = Diagnostic::coded(code, at, format!("unexpected character {}", shown(c)));
                match c {
                    '&' => d.with_fix(unsafe_fix("if you meant logical and, use `&&`", at, "&&")),
                    '|' => d.with_fix(unsafe_fix("if you meant logical or, use `||`", at, "||")),
                    '\u{FEFF}' if at.start() == 0 => d.with_fix(Fix {
                        message: "remove the byte order mark at the start of the file".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: at,
                            replacement: String::new(),
                        }],
                    }),
                    _ => d,
                }
            }
            DiagnosticKind::Expected { what, found, at } => {
                Diagnostic::coded(code, at, format!("expected {what}, found {found}"))
            }
            DiagnosticKind::Var { at, field } => {
                let d = Diagnostic::coded(code, at, "there is no `var` in Sisu");
                if field {
                    d.with_help("a field that can change is marked `mut`")
                } else {
                    d.with_help("Sisu declares a variable with `let mut`")
                }
            }
            DiagnosticKind::OptionalNests { ty, at } => {
                Diagnostic::coded(code, at, format!("`{ty}??` is not a type"))
                    .with_help("optionals do not nest")
            }
            DiagnosticKind::FieldComma {
                at,
                member,
                replacement,
            } => {
                let message = if replacement.is_empty() {
                    "remove the `,`"
                } else {
                    "replace the `,` with `;`"
                };
                Diagnostic::coded(
                    code,
                    at,
                    format!("{member}s are separated by a line break or `;`, not `,`"),
                )
                .with_help(format!("put each {member} on its own line"))
                .with_fix(Fix {
                    message: message.to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: at,
                        replacement,
                    }],
                })
            }
            DiagnosticKind::FieldLet { at, rest } => {
                let d = Diagnostic::coded(code, at, "a field takes no `let`");
                match rest {
                    LetField::Whole { remove } => d.with_fix(Fix {
                        message: "remove the `let`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: remove,
                            replacement: String::new(),
                        }],
                    }),
                    LetField::Other {
                        mutable: true,
                        field,
                    } => d.with_help(format!("drop the `let`: `mut {field}`")),
                    LetField::Other {
                        mutable: false,
                        field,
                    } => d.with_help(format!("write it bare: `{field}`")),
                }
            }
            DiagnosticKind::MethodNeedsSelf { at } => {
                Diagnostic::coded(code, at, "a method needs `self` as its first parameter")
                    .with_help("static methods are not supported yet")
            }
            DiagnosticKind::MutSelf {
                keyword,
                receiver,
                safe,
                owner,
            } => {
                let remove = Span::new(keyword.start(), receiver.start());
                match owner {
                    Owner::Class => Diagnostic::coded(
                        code,
                        keyword,
                        "a class method takes `self`, not `mut self`",
                    )
                    .with_help("an object is shared: `self` can already write every `mut` field")
                    .with_fix(Fix {
                        message: if safe {
                            "remove the `mut`"
                        } else {
                            "try removing the `mut`; the receiver may need another change"
                        }
                        .to_string(),
                        safe,
                        edits: vec![Edit {
                            span: remove,
                            replacement: String::new(),
                        }],
                    }),
                    // The body may still assign `self`, so the fix is never safe.
                    Owner::Enum(name) => Diagnostic::coded(
                        code,
                        keyword,
                        format!("`{name}` is an enum, and enum values never change"),
                    )
                    .with_help(format!("return a new `{name}`"))
                    .with_fix(unsafe_fix("remove the `mut`", remove, "")),
                }
            }
            DiagnosticKind::MutParameter { name, at } => {
                Diagnostic::coded(code, at, "a parameter cannot be `mut`").with_help(format!(
                    "copy it into a variable: `let mut {name} = {name}`"
                ))
            }
            DiagnosticKind::SelfNotFirst { at } => {
                Diagnostic::coded(code, at, "`self` is only the first parameter of a method")
            }
            DiagnosticKind::AssignToExpression { at } => {
                Diagnostic::coded(code, at, "cannot assign to this expression")
            }
            DiagnosticKind::AssignThroughSafeAccess { at } => {
                Diagnostic::coded(code, at, "cannot assign through `?.`")
                    .with_help("unwrap it with `if let`")
            }
            DiagnosticKind::RangeOutsideFor { at } => Diagnostic::coded(
                code,
                at,
                "ranges are only allowed in `for` and in patterns",
            ),
            DiagnosticKind::EqualityChain { at } => {
                Diagnostic::coded(code, at, "`==`, `!=` and `is` do not chain")
                    .with_help("join the comparisons with `&&`")
            }
            DiagnosticKind::ComparisonDirection { at } => {
                Diagnostic::coded(code, at, "a comparison chain must go in one direction")
            }
            DiagnosticKind::BuiltInName { name, at, kind } => Diagnostic::coded(
                code,
                at,
                match kind {
                    BuiltIn::Type => format!("`{name}` is a built-in type"),
                    BuiltIn::Constructor => format!("`{name}` is a built-in name"),
                    BuiltIn::Keyword => {
                        format!("`{name}` is a keyword; a variant cannot take its name")
                    }
                },
            ),
            DiagnosticKind::NeverConstructed { name, at, cause } => match cause {
                Unbuilt::Field { field, target, ty } => Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` can never be constructed: field `{field}` needs a value of type `{target}`"),
                )
                .with_fix(Fix {
                    message: format!("if the field can be absent, make it optional: `{target}?`"),
                    safe: false,
                    edits: vec![Edit {
                        span: Span::new(ty.end(), ty.end()),
                        replacement: "?".to_string(),
                    }],
                }),
                Unbuilt::Chain(links) => {
                    let chain = links
                        .iter()
                        .map(|(holder, held)| format!("`{holder}` holds a `{held}`"))
                        .collect::<Vec<_>>()
                        .join(", and ");
                    Diagnostic::coded(code, at, format!("no value of `{name}` can be built"))
                        .with_label(chain)
                }
            },
            DiagnosticKind::NoMain => {
                Diagnostic::coded(code, Span::new(0, 0), "no `main` function")
            }
            DiagnosticKind::MainSignature { at } => {
                Diagnostic::coded(
                    code,
                    at,
                    "`main` takes no parameters, or one `[string]`, and returns no value",
                )
            }
            DiagnosticKind::UnknownType {
                name,
                at,
                suggestion,
            } => similar(
                Diagnostic::coded(code, at, format!("cannot find type `{name}`")).with_help(
                    "the types are `i64`, `u8`, `f64`, `bool`, `char`, `string`, classes, enums and arrays such as `[i64]`",
                ),
                at,
                suggestion,
            ),
            DiagnosticKind::BodyHasNoValue { function, want, at } => Diagnostic::coded(
                code,
                at,
                format!("function `{function}` must return `{want}`, but its body has no value"),
            ),
            DiagnosticKind::Mismatch {
                want,
                got,
                at,
                source,
                fix,
                conversion,
            } => {
                let mut want: Vec<_> = want.iter().map(|ty| format!("`{ty}`")).collect();
                // `a`, `b` or `c`.
                let last = want.pop().expect("a mismatch should want at least one type");
                let want = if want.is_empty() {
                    last
                } else {
                    format!("{} or {last}", want.join(", "))
                };
                let mut d =
                    Diagnostic::coded(code, at, format!("expected {want}, found `{got}`"));
                match source {
                    Some(MismatchSource::Concat(operand)) => {
                        d = d.with_secondary(operand, "`+` joins this `string`");
                    }
                    Some(MismatchSource::Binding { name, at }) => {
                        d = d.with_secondary(at, format!("`{name}` is {want} here"));
                    }
                    None => {}
                }
                if conversion {
                    d = d.with_help(
                        "a conversion that can fail gives an optional: give a default with `??`, or unwrap it with `if let`",
                    );
                }
                match fix {
                    Some(MismatchFix::Convert(Conversion {
                        to,
                        through,
                        operand,
                    })) => d.with_fix(Fix {
                        message: format!("if the types are right, convert it with `{to}(...)`"),
                        safe: false,
                        edits: vec![
                            Edit {
                                span: Span::new(operand.start(), operand.start()),
                                replacement: through.map_or(format!("{to}("), |through| {
                                    format!("{to}({through}(")
                                }),
                            },
                            Edit {
                                span: Span::new(operand.end(), operand.end()),
                                replacement: ")".repeat(1 + usize::from(through.is_some())),
                            },
                        ],
                    }),
                    Some(MismatchFix::Interpolate { lhs, rhs }) => {
                        // The text between the operands goes, with a literal's quote beside it;
                        // any other operand is wrapped in braces, and the outer ones get quotes.
                        let (start, close) = if lhs.literal {
                            (lhs.span.end() - 1, "")
                        } else {
                            (lhs.span.end(), "}")
                        };
                        let (end, open) = if rhs.literal {
                            (rhs.span.start() + 1, "")
                        } else {
                            (rhs.span.start(), "{")
                        };
                        let mut edits = vec![Edit {
                            span: Span::new(start, end),
                            replacement: format!("{close}{open}"),
                        }];
                        if !lhs.literal {
                            edits.insert(
                                0,
                                Edit {
                                    span: Span::new(lhs.span.start(), lhs.span.start()),
                                    replacement: "\"{".to_string(),
                                },
                            );
                        }
                        if !rhs.literal {
                            edits.push(Edit {
                                span: Span::new(rhs.span.end(), rhs.span.end()),
                                replacement: "}\"".to_string(),
                            });
                        }
                        d.with_fix(Fix {
                            message: "use interpolation".to_string(),
                            safe: false,
                            edits,
                        })
                    }
                    // The call's name and parentheses become the literal's quote and brace.
                    Some(MismatchFix::InterpolateCall { call, arg }) => d.with_fix(Fix {
                        message: "use interpolation".to_string(),
                        safe: false,
                        edits: vec![
                            Edit {
                                span: Span::new(call.start(), arg.start()),
                                replacement: "\"{".to_string(),
                            },
                            Edit {
                                span: Span::new(arg.end(), call.end()),
                                replacement: "}\"".to_string(),
                            },
                        ],
                    }),
                    Some(MismatchFix::WrapInString(value)) => d.with_fix(Fix {
                        message: "use interpolation".to_string(),
                        safe: true,
                        edits: vec![
                            Edit {
                                span: Span::new(value.start(), value.start()),
                                replacement: "\"{".to_string(),
                            },
                            Edit {
                                span: Span::new(value.end(), value.end()),
                                replacement: "}\"".to_string(),
                            },
                        ],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::BindingNotFound {
                name,
                at,
                suggestion,
            } => similar(
                Diagnostic::coded(code, at, format!("cannot find binding `{name}`")),
                at,
                suggestion,
            ),
            DiagnosticKind::DefinedTwice {
                kind,
                name,
                at,
                first,
            } => {
                let (message, span) = match first {
                    Earlier::At {
                        span,
                        kind: first,
                        owner,
                    } if first != kind => {
                        let owner = owner.map_or_else(String::new, |c| format!(" of `{c}`"));
                        let (article, word) = (first.article(), first.word());
                        (format!("`{name}` is already {article} {word}{owner}"), Some(span))
                    }
                    Earlier::At { span, .. } => {
                        (format!("{} `{name}` is defined twice", kind.word()), Some(span))
                    }
                    Earlier::BuiltIn => (format!("`{name}` is a built-in function"), None),
                };
                let d = Diagnostic::coded(code, at, message);
                match span {
                    Some(span) => d.with_secondary(span, "first defined here"),
                    None => d,
                }
            }
            DiagnosticKind::ForNeedsArray { found, at } => Diagnostic::coded(
                code,
                at,
                format!("`for` needs an array or a range; this is `{found}`"),
            ),
            DiagnosticKind::ReturnNeedsValue { want, at } => {
                Diagnostic::coded(code, at, format!("`return` needs a value of type `{want}`"))
            }
            DiagnosticKind::UnexpectedReturnValue { at } => {
                Diagnostic::coded(code, at, "this function returns no value")
            }
            DiagnosticKind::JumpOutsideLoop { keyword, at } => {
                Diagnostic::coded(code, at, format!("`{keyword}` outside a loop"))
            }
            DiagnosticKind::AlreadyDeclared { name, at, first } => Diagnostic::coded(
                code,
                at,
                format!("`{name}` is already declared in this scope"),
            )
            .with_secondary(first, "first declared here"),
            DiagnosticKind::IntegerDoesNotFit { text, at } => {
                Diagnostic::coded(code, at, format!("`{text}` does not fit in `u8`"))
            }
            DiagnosticKind::NotExactlyF64 { text, nearest, at } => {
                Diagnostic::coded(code, at, format!("`{text}` is not exactly an `f64`")).with_fix(
                    Fix {
                        message: format!("write `{nearest}`"),
                        safe: true,
                        edits: vec![Edit {
                            span: at,
                            replacement: nearest,
                        }],
                    },
                )
            }
            DiagnosticKind::SelfOutsideMethod { at } => {
                Diagnostic::coded(code, at, "`self` is only available in a method")
            }
            DiagnosticKind::NegatedU8 { at } => {
                Diagnostic::coded(code, at, "`u8` cannot be negative")
            }
            DiagnosticKind::NoValue { at, never } => {
                let d = Diagnostic::coded(code, at, "expression has no value");
                if never {
                    d.with_label("never finishes")
                } else {
                    d
                }
            }
            DiagnosticKind::NeverNone { value, at } => Diagnostic::coded(
                code,
                at,
                format!("{} is not optional, so it is never `None`", named(value)),
            ),
            DiagnosticKind::IsTestsNone { tested, at } => {
                let d = Diagnostic::coded(code, at, "`is` tests identity, and `None` is not an object");
                match tested {
                    Some(path) => d.with_help(format!("use `{path} == None`")),
                    None => d,
                }
            }
            DiagnosticKind::IsNeedsObjects { found, at } => {
                Diagnostic::coded(code, at, format!("`is` needs objects; found `{found}`"))
                    .with_help("use `==` to compare values")
            }
            DiagnosticKind::CannotCompare { left, right, at } => {
                Diagnostic::coded(code, at, format!("cannot compare `{left}` with `{right}`"))
            }
            DiagnosticKind::CoalesceNotOptional { at } => {
                Diagnostic::coded(code, at, "the left side of `??` is not optional")
            }
            DiagnosticKind::SafeAccessNotOptional { value, at, access } => {
                Diagnostic::coded(code, at, format!("{} is not optional", named(value))).with_fix(
                    Fix {
                        message: "replace `?.` with `.`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: access,
                            replacement: ".".to_string(),
                        }],
                    },
                )
            }
            DiagnosticKind::CannotInfer { literal, at } => {
                let (literal, example) = match literal {
                    Literal::None => ("None", "let x: Tree? = None"),
                    Literal::EmptyArray => ("[]", "let xs: [i64] = []"),
                };
                Diagnostic::coded(code, at, format!("cannot infer the type of `{literal}`"))
                    .with_help(format!("write the type: `{example}`"))
            }
            DiagnosticKind::UnwrapNeedsOptional {
                keyword,
                found,
                at,
                compare,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!(
                        "`{}` needs an enum or an optional; this is `{found}`",
                        keyword.word()
                    ),
                );
                if compare {
                    d.with_help("compare with `==`")
                } else {
                    d
                }
            }
            DiagnosticKind::FunctionNotFound {
                name,
                at,
                suggestion,
            } => similar(
                Diagnostic::coded(code, at, format!("cannot find function `{name}`")),
                at,
                suggestion,
            ),
            DiagnosticKind::ArgumentCount {
                name,
                want,
                found,
                at,
            } => {
                let plural = if want == 1 { "" } else { "s" };
                Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` takes {want} argument{plural}, found {found}"),
                )
            }
            DiagnosticKind::AlreadyConverted {
                value,
                ty,
                at,
                conversion,
                name,
                grouped,
                labeled,
                commented,
            } => {
                let edit = |span: Span, replacement: &str| Edit {
                    span,
                    replacement: replacement.to_string(),
                };
                // A grouped value keeps the `)` after it, so only `T(` and what sits before the
                // value becomes its `(`: parentheses around the call stay paired. Otherwise the
                // head and the tail go, with those parentheses, in pairs.
                let edits = if grouped {
                    vec![edit(Span::new(name.start(), at.start()), "(")]
                } else {
                    vec![
                        edit(Span::new(conversion.start(), at.start()), ""),
                        edit(Span::new(at.end(), conversion.end()), ""),
                    ]
                };
                let d = Diagnostic::coded(code, at, format!("{} is already `{ty}`", named(value)));
                if labeled {
                    return d;
                }
                d.with_fix(Fix {
                    message: if commented {
                        "remove the conversion, and the comments inside it".to_string()
                    } else {
                        "remove the conversion".to_string()
                    },
                    safe: !commented,
                    edits,
                })
            }
            DiagnosticKind::LabeledArgument { at, variant: None } => {
                Diagnostic::coded(code, at, "labels on arguments come in milestone 10")
            }
            DiagnosticKind::LabeledArgument {
                at,
                variant: Some(VariantLabels { path, labels }),
            } => Diagnostic::coded(code, at, format!("`{path}` takes no labels")).with_fix(Fix {
                message: "remove the labels".to_string(),
                safe: false,
                edits: labels
                    .into_iter()
                    .map(|span| Edit {
                        span,
                        replacement: String::new(),
                    })
                    .collect(),
            }),
            DiagnosticKind::MissingField { field, fields, at } => {
                Diagnostic::coded(code, at, format!("missing field `{field}`"))
                    .with_help(field_order(&fields))
            }
            DiagnosticKind::MethodNotField {
                name,
                owner,
                at,
                call,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` is a method of `{owner}`, not a field"),
                );
                match call {
                    Some(safe) => d.with_fix(Fix {
                        message: if safe {
                            format!("call it: `{name}()`")
                        } else {
                            format!("if you meant to call `{name}`, add `()`; it also needs its arguments")
                        },
                        safe,
                        edits: vec![Edit {
                            span: Span::new(at.end(), at.end()),
                            replacement: "()".to_string(),
                        }],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::FieldNotMethod { name, owner, at } => Diagnostic::coded(
                code,
                at,
                format!("`{name}` is a field of `{owner}`, not a method"),
            ),
            DiagnosticKind::PrintArgument {
                function,
                at,
                class,
            } => {
                let takes = format!("`{function}` takes one `i64`, `u8`, `f64`, `bool`, `char` or `string`");
                match class {
                    Some(class) => {
                        Diagnostic::coded(code, at, format!("cannot {function} `{class}`"))
                            .with_help(takes)
                    }
                    None => Diagnostic::coded(code, at, takes),
                }
            }
            DiagnosticKind::PrintArray {
                function,
                ty,
                optional,
                at,
            } => {
                let help = if optional {
                    format!("unwrap it with `if let`, then {function} each element in a `for` loop")
                } else {
                    format!("{function} each element in a `for` loop")
                };
                Diagnostic::coded(code, at, format!("cannot {function} `{ty}`")).with_help(help)
            }
            DiagnosticKind::PrintNever { function, at } => Diagnostic::coded(
                code,
                at,
                format!("`{function}` never runs: its argument never finishes"),
            ),
            DiagnosticKind::MayBeNone { value, at, access } => {
                let d = Diagnostic::coded(code, at, format!("{} may be `None`", named(value)));
                match access {
                    Some(access) => d
                        .with_help("to use the value only when it is present, unwrap it with `if let`")
                        .with_fix(unsafe_fix("if a `None` result is fine, use `?.`", access, "?.")),
                    None => d.with_help("unwrap it with `if let`"),
                }
            }
            DiagnosticKind::NoMember {
                owner,
                kind,
                name,
                at,
                suggestion,
            } => similar(
                Diagnostic::coded(
                    code,
                    at,
                    format!("`{owner}` has no {} `{name}`", kind.word()),
                ),
                at,
                suggestion,
            ),
            DiagnosticKind::TooManyArguments { class, fields, at } => {
                let plural = if fields == 1 { "" } else { "s" };
                Diagnostic::coded(
                    code,
                    at,
                    format!("too many arguments: `{class}` has {fields} field{plural}"),
                )
            }
            DiagnosticKind::ArgumentNeedsLabel { field, at } => {
                Diagnostic::coded(code, at, "this argument needs a label").with_fix(Fix {
                    message: "label it with its field".to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: Span::new(at.start(), at.start()),
                        replacement: format!("{field}: "),
                    }],
                })
            }
            DiagnosticKind::FieldOutOfOrder {
                field,
                found,
                fields,
                at,
            } => Diagnostic::coded(
                code,
                at,
                format!("expected field `{field}` here, found `{found}`"),
            )
            .with_help(field_order(&fields)),
            DiagnosticKind::SameObject { class, at } => Diagnostic::coded(
                code,
                at,
                format!("every element would be the same `{class}` object"),
            )
            .with_help(format!(
                "start from `[None; n]` with type `[{class}?]` and fill it in a loop"
            )),
            DiagnosticKind::CannotIndex { ty, at } => {
                Diagnostic::coded(code, at, format!("cannot index `{ty}`"))
                    .with_help("only arrays can be indexed")
            }
            DiagnosticKind::SumNeedsNumbers { ty, at } => {
                Diagnostic::coded(code, at, format!("`sum` needs an `[i64]` or an `[f64]`, found `{ty}`"))
            }
            DiagnosticKind::MethodAsFunction {
                name,
                at,
                receiver,
                next,
                wrap,
                safe,
            } => {
                let (open, close) = if wrap { ("(", ")") } else { ("", "") };
                Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` is a method, not a function"),
                )
                .with_fix(Fix {
                    message: if safe {
                        format!("call `{name}` as a method")
                    } else {
                        format!("try calling `{name}` as a method, then fix its arguments")
                    },
                    safe,
                    edits: vec![
                        Edit {
                            span: Span::new(at.start(), receiver.start()),
                            replacement: open.to_string(),
                        },
                        Edit {
                            span: Span::new(receiver.end(), next.map_or(receiver.end(), Span::start)),
                            replacement: format!("{close}.{name}("),
                        },
                    ],
                })
            }
            DiagnosticKind::NestingTooDeep { at, limit } => {
                Diagnostic::coded(code, at, "nesting is too deep")
                    .with_help(format!("the limit is {limit} levels"))
            }
            DiagnosticKind::InvalidUtf8 { at } => {
                Diagnostic::coded(code, at, "source is not valid UTF-8")
                    .with_help("save the file as UTF-8")
            }
            DiagnosticKind::AssignToSelf { at } => {
                Diagnostic::coded(code, at, "cannot assign to `self`")
                    .with_help("an object is shared: `self` can already write every `mut` field")
            }
            DiagnosticKind::AssignToFunction { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is a function, not a variable"))
            }
            DiagnosticKind::MutOnUnderscore { at, keyword } => {
                Diagnostic::coded(code, at, "`_` binds nothing, so it cannot be `mut`").with_fix(
                    Fix {
                        message: "use `let _`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: Span::new(keyword.start(), at.start()),
                            replacement: String::new(),
                        }],
                    },
                )
            }
            DiagnosticKind::UnderscoreValue { at, bind } => {
                let d = Diagnostic::coded(code, at, "`_` is not a value");
                if bind {
                    d.with_fix(Fix {
                        message: "bind it with `let _`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: Span::new(at.start(), at.start()),
                            replacement: "let ".to_string(),
                        }],
                    })
                } else {
                    d
                }
            }
            DiagnosticKind::UnderscoreName { at } => {
                Diagnostic::coded(code, at, "`_` cannot be a name")
            }
            DiagnosticKind::BlockAsStatement { at } => {
                Diagnostic::coded(code, at, "a block cannot stand alone as a statement")
                    .with_help("bind its value: `let x = { ... }`")
            }
            DiagnosticKind::JumpInLoopHeader { keyword, at, loop_at } => {
                Diagnostic::coded(code, at, format!("`{keyword}` cannot appear in a loop header"))
                    .with_secondary(loop_at, "the header of this loop")
            }
            DiagnosticKind::ReservedWord { word, at } => Diagnostic::coded(
                code,
                at,
                format!("`{word}` is reserved for a later version of Sisu"),
            ),
            DiagnosticKind::ConstInFunction { at } => {
                Diagnostic::coded(code, at, "`const` is declared at the top level").with_fix(Fix {
                    message: "use `let`".to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: at,
                        replacement: "let".to_string(),
                    }],
                })
            }
            DiagnosticKind::ConstantType { at } => {
                Diagnostic::coded(code, at, "a constant is `i64`, `u8`, `f64`, `bool`, `char` or `string`")
            }
            DiagnosticKind::ConstantNotKnown { at, interpolation } => if interpolation {
                Diagnostic::coded(code, at, "a constant's value cannot use interpolation")
                    .with_help("join `string` constants with `+`, or use a `let` in a function")
            } else {
                Diagnostic::coded(code, at, "a constant's value must be known at compile time")
                    .with_help("use literals, operators and earlier constants")
            },
            DiagnosticKind::ConstantOrder {
                name,
                user,
                at,
                declared,
                itself,
            } => if itself {
                Diagnostic::coded(code, at, format!("`{name}` cannot use itself"))
            } else {
                Diagnostic::coded(code, at, format!("`{name}` is declared below `{user}`"))
                    .with_help(format!("move `{name}` above `{user}`"))
            }
            .with_secondary(declared, "declared here"),
            DiagnosticKind::ConstantPanics { panic, at } => {
                Diagnostic::coded(code, at, panic.message())
            }
            DiagnosticKind::UnknownEscape { found, at, literal } => {
                let escape = if invisible(found) {
                    format!("`\\` followed by {}", shown(found))
                } else {
                    format!("`\\{found}`")
                };
                let diagnostic = Diagnostic::coded(code, at, format!("unknown escape {escape}"))
                    .with_help(
                        r#"the escapes are `\n`, `\t`, `\r`, `\0`, `\\`, `\"`, `\'`, `\{`, `\}` and `\u{...}`"#,
                    );
                // In a `char` literal a second backslash would make two code points.
                match literal {
                    LiteralKind::String => diagnostic.with_fix(unsafe_fix(
                        r"if you meant a backslash, write `\\`",
                        Span::new(at.start(), at.start()),
                        "\\",
                    )),
                    LiteralKind::Char => diagnostic,
                }
            }
            DiagnosticKind::MalformedUnicodeEscape { at } => {
                Diagnostic::coded(code, at, r"malformed `\u` escape")
                    .with_help(r"write `\u{` and 1 to 6 hex digits, then `}`")
            }
            DiagnosticKind::NotScalarValue { escape, at } => {
                Diagnostic::coded(code, at, format!("`{escape}` is not a Unicode scalar value"))
                    .with_help("scalar values run from 0 to 10FFFF, without D800 to DFFF")
            }
            DiagnosticKind::NotOneCodePoint { at, string } => {
                let d = Diagnostic::coded(code, at, "a `char` holds one code point");
                if string {
                    let quote = |at| Edit {
                        span: Span::new(at, at + 1),
                        replacement: "\"".to_string(),
                    };
                    d.with_fix(Fix {
                        message: "if you meant a string, write it in double quotes".to_string(),
                        safe: false,
                        edits: vec![quote(at.start()), quote(at.end() - 1)],
                    })
                } else {
                    d
                }
            }
            DiagnosticKind::LiteralNotClosed {
                kind,
                at,
                interpolation,
                end_of_file,
            } => match kind {
                LiteralKind::Char => Diagnostic::coded(code, at, "`char` literal is not closed"),
                LiteralKind::String => {
                    let d = Diagnostic::coded(code, at, "string literal is not closed");
                    if interpolation {
                        d.with_help(r"a `{` starts an interpolation: end it with `}`, or write `\{` for a literal brace")
                    } else if end_of_file {
                        d
                    } else {
                        d.with_help(r"write `\n` for a line break")
                    }
                }
            },
            DiagnosticKind::ArithmeticOnChar { at } => {
                Diagnostic::coded(code, at, "arithmetic does not apply to `char`").with_help(
                    "convert through `i64`, then give a default: `char(i64(c) + 1) ?? c`",
                )
            }
            DiagnosticKind::BraceInString { at } => {
                Diagnostic::coded(code, at, "a `}` in a string must be escaped").with_fix(Fix {
                    message: r"write `\}`".to_string(),
                    safe: true,
                    edits: vec![backslash(at.start())],
                })
            }
            DiagnosticKind::EmptyInterpolation { at } => {
                Diagnostic::coded(code, at, "empty interpolation").with_fix(Fix {
                    message: r"write `\{\}` for literal braces".to_string(),
                    safe: false,
                    edits: vec![backslash(at.start()), backslash(at.end() - 1)],
                })
            }
            DiagnosticKind::CommentInInterpolation { at } => {
                Diagnostic::coded(code, at, "a comment cannot appear inside an interpolation")
            }
            DiagnosticKind::CannotInterpolate { ty, help, at } => {
                let d = Diagnostic::coded(code, at, format!("cannot interpolate `{ty}`"));
                match help {
                    Some(InterpolateHelp::ForLoop) => d.with_help("build the text in a `for` loop"),
                    Some(InterpolateHelp::Unwrap) => {
                        d.with_help("unwrap it with `if let`, or give a default with `??`")
                    }
                    None => d,
                }
            }
            DiagnosticKind::TodoLeft { at } => {
                Diagnostic::coded(code, at, "`todo` panics when it runs")
            }
            DiagnosticKind::TodoForm { at, delete, safe } => Diagnostic::coded(
                code,
                at,
                "`todo` takes one message in parentheses, or none",
            )
            .with_fix(Fix {
                message: "remove it".to_string(),
                safe,
                edits: vec![Edit {
                    span: delete,
                    replacement: String::new(),
                }],
            }),
            DiagnosticKind::ConversionInConstant { to, at, fix } => {
                let d = Diagnostic::coded(code, at, "a constant cannot use a conversion that can fail")
                    .with_label(format!(
                        "`{to}(...)` gives {} `{to}?`",
                        if to == "i64" { "an" } else { "a" }
                    ))
                    .with_help("write the literal with its type: `const B: u8 = 200`");
                match fix {
                    Some(TypedLiteral {
                        call,
                        literal,
                        annotate,
                    }) => {
                        let mut edits: Vec<Edit> = annotate
                            .into_iter()
                            .map(|(at, ty)| Edit {
                                span: Span::new(at, at),
                                replacement: format!(": {ty}"),
                            })
                            .collect();
                        edits.push(Edit {
                            span: call,
                            replacement: literal,
                        });
                        d.with_fix(Fix {
                            message: "write the literal with its type".to_string(),
                            safe: true,
                            edits,
                        })
                    }
                    None => d,
                }
            }
            DiagnosticKind::UnusedValue { ty, at, arm, sites } => {
                let d = Diagnostic::coded(code, at, format!("unused value of type `{ty}`"));
                let help = if arm {
                    "use it, or write `{ let _ = ... }` to drop it on purpose"
                } else {
                    "use it, or write `let _ =` to drop it on purpose"
                };
                let mut edits = Vec::new();
                for (site, arm) in sites {
                    let start = Span::new(site.start(), site.start());
                    if arm {
                        edits.push(Edit {
                            span: start,
                            replacement: "{ let _ = ".to_string(),
                        });
                        edits.push(Edit {
                            span: Span::new(site.end(), site.end()),
                            replacement: " }".to_string(),
                        });
                    } else {
                        edits.push(Edit {
                            span: start,
                            replacement: "let _ = ".to_string(),
                        });
                    }
                }
                let d = d.with_help(help);
                if edits.is_empty() {
                    return d;
                }
                d.with_fix(Fix {
                    message: "drop it on purpose".to_string(),
                    safe: false,
                    edits,
                })
            }
            DiagnosticKind::TypeAsValue { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is a type, not a value"))
            }
            DiagnosticKind::MathIdiom { written, fix } => {
                let d = Diagnostic::coded(
                    code,
                    written,
                    "math in Sisu is methods and constants on `f64`",
                );
                let Some(fix) = fix else { return d };
                let (form, edits) = match fix {
                    MathFix::Constant { name } => (
                        format!("f64.{name}"),
                        vec![Edit {
                            span: written,
                            replacement: format!("f64.{name}"),
                        }],
                    ),
                    MathFix::Method {
                        name,
                        form,
                        receiver,
                        next,
                        wrap,
                    } => {
                        let (open, close) = if wrap { ("(", ")") } else { ("", "") };
                        let edits = vec![
                            Edit {
                                span: Span::new(written.start(), receiver.start()),
                                replacement: open.to_string(),
                            },
                            Edit {
                                span: Span::new(
                                    receiver.end(),
                                    next.map_or(receiver.end(), Span::start),
                                ),
                                replacement: format!("{close}.{name}("),
                            },
                        ];
                        (form, edits)
                    }
                };
                d.with_fix(Fix {
                    message: format!("write `{form}`"),
                    safe: true,
                    edits,
                })
            }
            DiagnosticKind::PowerOperator { at, fix } => {
                let d = Diagnostic::coded(code, at, "Sisu has no `**`").with_help("use `x.pow(y)`");
                match fix {
                    Some(PowerFix { left, right }) => d.with_fix(Fix {
                        message: "try a `pow` call, which needs an `f64` on the left".to_string(),
                        safe: false,
                        edits: vec![
                            Edit {
                                span: Span::new(left.end(), right.start()),
                                replacement: ".pow(".to_string(),
                            },
                            Edit {
                                span: Span::new(right.end(), right.end()),
                                replacement: ")".to_string(),
                            },
                        ],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::MalformedFormatSpec { at } => {
                Diagnostic::coded(code, at, "a format spec is `[<^>][width][.precision]`")
            }
            DiagnosticKind::UnsupportedFormatSpec { at } => Diagnostic::coded(
                code,
                at,
                "Sisu's format specs take only alignment, width and precision",
            ),
            DiagnosticKind::FormatSpecLimit { at, width } => Diagnostic::coded(
                code,
                at,
                if width {
                    "a width is at most 1000"
                } else {
                    "a precision is at most 30"
                },
            ),
            DiagnosticKind::PrecisionNeedsF64 { ty, at, fix } => {
                let d = Diagnostic::coded(code, at, "precision needs an `f64`")
                    .with_label(format!("the value is `{ty}`"));
                match fix {
                    Some(operand) => d.with_fix(Fix {
                        message: "convert it with `f64(...)`".to_string(),
                        safe: true,
                        edits: vec![
                            Edit {
                                span: Span::new(operand.start(), operand.start()),
                                replacement: "f64(".to_string(),
                            },
                            Edit {
                                span: Span::new(operand.end(), operand.end()),
                                replacement: ")".to_string(),
                            },
                        ],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::VariantAfterMethod { at, first_method } => {
                Diagnostic::coded(code, at, "variants come before methods")
                    .with_secondary(first_method, "the first method")
            }
            DiagnosticKind::VariantCase { name, at } => {
                let message = "a variant name starts with an upper-case letter";
                // Only a lower-case first letter gets a fix; `_Foo` would still break the rule.
                let mut chars = name.chars();
                let capitalized = chars
                    .next()
                    .filter(|c| c.is_lowercase())
                    .map(|c| format!("{}{}", c.to_uppercase(), chars.as_str()));
                match capitalized {
                    Some(capitalized) => {
                        Diagnostic::coded(code, at, format!("{message}: `{capitalized}`"))
                            .with_fix(unsafe_fix("capitalize it", at, &capitalized))
                    }
                    None => Diagnostic::coded(code, at, message),
                }
            }
            DiagnosticKind::MixedLabels { variant, at } => Diagnostic::coded(
                code,
                at,
                format!("label every field of `{variant}` or none"),
            ),
            DiagnosticKind::EmptyEnum { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` needs at least one variant"))
            }
            DiagnosticKind::EnumNamedLocal { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` names an enum"))
            }
            DiagnosticKind::BareVariant { name, enums, at } => {
                // Inserting the enum's name is certain only when one enum has the variant.
                let safe = enums.len() == 1;
                let qualify = |e: &String| Fix {
                    message: format!("write `{e}.{name}`"),
                    safe,
                    edits: vec![Edit {
                        span: Span::new(at.start(), at.start()),
                        replacement: format!("{e}."),
                    }],
                };
                let d = match enums.as_slice() {
                    [e] => Diagnostic::coded(
                        code,
                        at,
                        format!("`{name}` is a variant of `{e}`; write `{e}.{name}`"),
                    ),
                    several => {
                        let paths: Vec<_> =
                            several.iter().map(|e| format!("`{e}.{name}`")).collect();
                        Diagnostic::coded(
                            code,
                            at,
                            format!("`{name}` is a variant of several enums"),
                        )
                        .with_help(format!("write one of {}", paths.join(", ")))
                    }
                };
                enums.iter().fold(d, |d, e| d.with_fix(qualify(e)))
            }
            DiagnosticKind::UnknownVariant {
                enum_name,
                name,
                at,
                suggestion,
            } => similar(
                Diagnostic::coded(code, at, format!("`{enum_name}` has no variant `{name}`")),
                at,
                suggestion,
            ),
            DiagnosticKind::VariantForm {
                path,
                payload,
                held,
                at,
            } => {
                if payload {
                    Diagnostic::coded(
                        code,
                        at,
                        format!("`{path}` needs its payload: `{path}(...)`"),
                    )
                } else {
                    Diagnostic::coded(code, at, format!("`{path}` holds nothing; write `{path}`"))
                        .with_fix(Fix {
                            message: "remove the parentheses".to_string(),
                            // Arguments or a comment would be deleted with the parentheses.
                            safe: !held,
                            edits: vec![Edit {
                                span: at,
                                replacement: String::new(),
                            }],
                        })
                }
            }
            DiagnosticKind::MemberKind {
                name,
                enum_name,
                method,
                at,
            } => {
                let message = if method {
                    format!("`{name}` is a method; call it on a value: `e.{name}()`")
                } else {
                    format!("`{name}` is a variant; build it with `{enum_name}.{name}`")
                };
                Diagnostic::coded(code, at, message)
            }
            DiagnosticKind::EnumField { enum_name, at } => Diagnostic::coded(
                code,
                at,
                format!("`{enum_name}` has no fields; take it apart with `match`"),
            ),
            DiagnosticKind::MatchIdiom { form, at } => match form {
                MatchIdiomForm::Switch => Diagnostic::coded(code, at, "Sisu writes `match`")
                    .with_fix(unsafe_fix("write `match`", at, "match")),
                MatchIdiomForm::Case { remove } => Diagnostic::coded(
                    code,
                    at,
                    "an arm is `pattern => body`, with no `case`",
                )
                .with_fix(Fix {
                    message: "remove the `case`".to_string(),
                    safe: false,
                    edits: remove
                        .into_iter()
                        .map(|span| Edit {
                            span,
                            replacement: String::new(),
                        })
                        .collect(),
                }),
                MatchIdiomForm::Default => Diagnostic::coded(code, at, "write `_ =>`")
                    .with_fix(safe_fix("write `_ =>`", at, "_ =>")),
                MatchIdiomForm::Else => Diagnostic::coded(code, at, "write `_`")
                    .with_fix(safe_fix("write `_`", at, "_")),
                MatchIdiomForm::Arrow => Diagnostic::coded(code, at, "an arm uses `=>`")
                    .with_fix(safe_fix("write `=>`", at, "=>")),
                MatchIdiomForm::EnumClass { name, remove } => {
                    Diagnostic::coded(code, at, format!("write `enum {name}`"))
                        .with_fix(safe_fix("remove the `class`", remove, ""))
                }
                MatchIdiomForm::Path { name, member } => {
                    Diagnostic::coded(code, at, format!("write `{name}.{member}`"))
                        .with_fix(safe_fix("write `.`", at, "."))
                }
            },
            DiagnosticKind::PatternForm { form, at } => match form {
                ForeignPattern::Labels { form, labels } => Diagnostic::coded(
                    code,
                    at,
                    format!("a pattern binds by position: `{form}`"),
                )
                .with_fix(Fix {
                    message: "remove the labels".to_string(),
                    // Labels out of order would swap the bindings.
                    safe: false,
                    edits: labels
                        .into_iter()
                        .map(|span| Edit {
                            span,
                            replacement: String::new(),
                        })
                        .collect(),
                }),
                ForeignPattern::Braces {
                    form,
                    replace,
                    fields,
                } => Diagnostic::coded(code, at, format!("write `{form}`")).with_fix(unsafe_fix(
                    &format!("write `{form}`"),
                    replace,
                    &fields,
                )),
            },
            DiagnosticKind::CannotMatch { ty, at } => {
                Diagnostic::coded(code, at, format!("cannot `match` on `{ty}`"))
                    .with_help("`match` takes enums, integers, `bool`, `char` and `string`")
            }
            DiagnosticKind::NotVariantHere { name, at } => {
                let mut lower = name.clone();
                // A name that is not a variant here starts with an upper-case letter.
                if let Some(first) = name.chars().next() {
                    lower.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
                }
                Diagnostic::coded(code, at, format!("`{name}` is not a variant here"))
                    .with_help("a binding starts with a lower-case letter")
                    .with_fix(unsafe_fix(&format!("write `{lower}`"), at, &lower))
            }
            DiagnosticKind::ConstantInPattern { name, at, guard } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` is a constant; a pattern cannot compare with it"),
                );
                match guard {
                    Some(GuardHelp::Rewritten(guard)) => {
                        d.with_help(format!("use a guard: `{guard}`"))
                    }
                    Some(GuardHelp::InWords) => d.with_help(format!(
                        "use a guard: bind a name and compare it with `{name}`"
                    )),
                    None => d,
                }
            }
            DiagnosticKind::OnlyNamePosition { ty, at } => Diagnostic::coded(
                code,
                at,
                format!("a `{ty}` position takes only a name or `_`"),
            ),
            DiagnosticKind::SubpatternCount {
                variant,
                fields,
                found,
                at,
            } => {
                let plural = if fields == 1 { "" } else { "s" };
                Diagnostic::coded(
                    code,
                    at,
                    format!("`{variant}` has {fields} field{plural}, the pattern has {found}"),
                )
            }
            DiagnosticKind::BoundTwice {
                name,
                at,
                first,
                guard,
            } => {
                let d =
                    Diagnostic::coded(code, at, format!("`{name}` is bound twice in this pattern"))
                        .with_secondary(first, "first bound here");
                match guard {
                    Some(GuardHelp::Rewritten(guard)) => {
                        d.with_help(format!("use a guard: `{guard}`"))
                    }
                    Some(GuardHelp::InWords) => d.with_help(format!(
                        "use a guard: bind a second name and compare it with `{name}`"
                    )),
                    None => d,
                }
            }
            DiagnosticKind::PatternMismatch { want, found, at } => Diagnostic::coded(
                code,
                at,
                format!("expected a pattern for `{want}`, found `{found}`"),
            ),
            DiagnosticKind::NonExhaustive {
                missing,
                partly,
                guarded,
                gap,
                at,
                insert,
                indent,
                close,
            } => {
                let d = Diagnostic::coded(code, at, format!("`match` does not cover `{missing}`"));
                let d = if guarded {
                    d.with_label("a guarded arm may not match")
                } else {
                    d
                };
                let d = if gap { d.with_help("add `_ =>`") } else { d };
                let d = match partly {
                    Some((arm, pattern, payload)) => {
                        d.with_secondary(arm, format!("`{pattern}` covers only `{payload}`"))
                    }
                    None => d,
                };
                let arm = match close {
                    Some(close) => format!("\n{indent}_ => todo\n{close}"),
                    None => format!("{indent}_ => todo\n"),
                };
                d.with_fix(unsafe_fix("add a `_ =>` arm", insert, &arm))
            }
            DiagnosticKind::UnreachableArm {
                at,
                alternative,
                covered_by,
                remove,
            } => {
                let what = if alternative { "alternative" } else { "arm" };
                let d = Diagnostic::coded(code, at, format!("unreachable {what}"));
                match covered_by {
                    Some(arm) => d.with_secondary(arm, "this arm covers it"),
                    None => d,
                }
                .with_fix(unsafe_fix(&format!("remove the {what}"), remove, ""))
            }
            DiagnosticKind::RedundantSemicolon { at, space } => {
                Diagnostic::coded(code, at, "unnecessary `;`").with_fix(safe_fix(
                    "remove the `;`",
                    at,
                    if space { " " } else { "" },
                ))
            }
            DiagnosticKind::AlwaysMatches { keyword, at } => match keyword {
                Unwrap::IfLet => Diagnostic::coded(code, at, "`if let` always matches here")
                    .with_help("use `match`"),
                Unwrap::WhileLet => Diagnostic::coded(
                    code,
                    at,
                    "`while let` always matches here, so the pattern never ends the loop",
                ),
            },
            DiagnosticKind::ForeignRange { form, at } => match form {
                ForeignRange::Exclusive { start, end, remove } => {
                    let end = end.as_deref().unwrap_or("end");
                    let d = Diagnostic::coded(code, at, format!("write `{start}..{end}`"));
                    match remove {
                        Some(lt) => d.with_fix(safe_fix("remove the `<`", lt, "")),
                        None => d,
                    }
                }
                ForeignRange::Ellipsis { start, end } => {
                    let end = end.as_deref().unwrap_or("end");
                    Diagnostic::coded(
                    code,
                    at,
                    format!(
                        "write `{start}..={end}` to include `{end}`, or `{start}..{end}` to leave it out"
                    ),
                    )
                }
            },
            DiagnosticKind::ArmSplit { at, join } => {
                let d = Diagnostic::coded(code, at, "an arm continues on the line above");
                match join {
                    Some(join) => d.with_fix(safe_fix("join it to the line above", join, " ")),
                    None => d,
                }
            }
            DiagnosticKind::InterpolatedPattern { at } => {
                Diagnostic::coded(code, at, "a pattern takes no interpolation")
            }
            DiagnosticKind::BadRange { form, at } => match form {
                BadRange::Type => {
                    Diagnostic::coded(code, at, "a range pattern takes `i64`, `u8` or `char`")
                }
                BadRange::Empty => Diagnostic::coded(code, at, "this range matches no value"),
            },
            DiagnosticKind::AlternativeBindings { name, at } => Diagnostic::coded(
                code,
                at,
                format!("`{name}` is bound in one alternative but not the other"),
            ),
            DiagnosticKind::NanComparison { at, negated, fix } => {
                let always = if negated { "true" } else { "false" };
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("comparison with `f64.nan` is always {always}"),
                )
                .with_help("`nan` is not equal to any number, itself included; test for it with `is_nan()`");
                match fix {
                    Some(NanFix { other, wrap }) => {
                        let (open, close) = if wrap { ("(", ")") } else { ("", "") };
                        let test = format!("{close}.is_nan()");
                        let not = if negated { "!" } else { "" };
                        let mut edits = if other.start() == at.start() {
                            // `other == f64.nan`: the rest of the comparison becomes the call.
                            vec![
                                Edit {
                                    span: Span::new(at.start(), at.start()),
                                    replacement: format!("{not}{open}"),
                                },
                                Edit {
                                    span: Span::new(other.end(), at.end()),
                                    replacement: test,
                                },
                            ]
                        } else {
                            // `f64.nan == other`: what comes before `other` is dropped.
                            vec![
                                Edit {
                                    span: Span::new(at.start(), other.start()),
                                    replacement: format!("{not}{open}"),
                                },
                                Edit {
                                    span: Span::new(other.end(), other.end()),
                                    replacement: test,
                                },
                            ]
                        };
                        // `x == f64.nan` writes nothing before `x`.
                        edits.retain(|e| e.span.start() != e.span.end() || !e.replacement.is_empty());
                        d.with_fix(Fix {
                            message: "test it with `is_nan()`".to_string(),
                            safe: true,
                            edits,
                        })
                    }
                    None => d,
                }
            }
        }
    }
}

/// The edit that writes a `\` at `at`.
fn backslash(at: usize) -> Edit {
    Edit {
        span: Span::new(at, at),
        replacement: "\\".to_string(),
    }
}

/// The help that lists a class's `fields` in declaration order.
fn field_order(fields: &[String]) -> String {
    let names: Vec<_> = fields.iter().map(|f| format!("`{f}`")).collect();
    format!("name the fields in declaration order: {}", names.join(", "))
}

/// A value as a message names it: its `path` in backticks, or "this value" when it has none.
fn named(path: Option<String>) -> String {
    path.map_or_else(|| "this value".to_string(), |path| format!("`{path}`"))
}

/// The fix that replaces the gap of `join` with a space: `message`, or `unsafe_message` when `join`
/// is not safe.
fn join_fix(message: &str, unsafe_message: &str, join: Join) -> Fix {
    Fix {
        message: if join.safe { message } else { unsafe_message }.to_string(),
        safe: join.safe,
        edits: vec![Edit {
            span: join.gap,
            replacement: " ".to_string(),
        }],
    }
}

/// `d`, with the unsafe fix that replaces the name at `at` with `suggestion`, if there is one.
fn similar(d: Diagnostic, at: Span, suggestion: Option<Suggestion>) -> Diagnostic {
    let Some(Suggestion { kind, name }) = suggestion else {
        return d;
    };
    d.with_fix(unsafe_fix(
        &format!(
            "{} {} with a similar name exists: `{name}`",
            kind.article(),
            kind.word()
        ),
        at,
        &name,
    ))
}

/// Whether `c` is a character the user cannot see.
pub(crate) fn invisible(c: char) -> bool {
    c.is_whitespace() || c.is_control() || is_format(c)
}

/// `c` as a message names it: a character the user cannot see by its code point, not printed.
fn shown(c: char) -> String {
    if invisible(c) {
        format!("U+{:04X}", u32::from(c))
    } else {
        format!("`{c}`")
    }
}

/// Whether `c` is in Unicode category Cf (format). Neither the standard library nor a dependency
/// has the category, so these are the Cf ranges of Unicode 17.0 (`DerivedGeneralCategory.txt`),
/// the `char::UNICODE_VERSION` of the pinned toolchain. Recheck the list on a toolchain upgrade;
/// a new Cf character prints raw until it is extended. `core` has a private
/// `char::is_format_control`; replace the list with it if it is ever stabilized.
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{AD}' | '\u{600}'..='\u{605}' | '\u{61C}' | '\u{6DD}' | '\u{70F}'
        | '\u{890}'..='\u{891}' | '\u{8E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// The unsafe fix `message` that replaces `span` with `replacement`.
/// The safe fix `message` that replaces `span` with `replacement`.
fn safe_fix(message: &str, span: Span, replacement: &str) -> Fix {
    Fix {
        safe: true,
        ..unsafe_fix(message, span, replacement)
    }
}

fn unsafe_fix(message: &str, span: Span, replacement: &str) -> Fix {
    Fix {
        message: message.to_string(),
        safe: false,
        edits: vec![Edit {
            span,
            replacement: replacement.to_string(),
        }],
    }
}

/// The safe fix `message` that writes `mut ` before the name at `declared`.
fn insert_mut(message: &str, declared: Span) -> Fix {
    Fix {
        message: message.to_string(),
        safe: true,
        edits: vec![Edit {
            span: Span::new(declared.start(), declared.start()),
            replacement: "mut ".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_parses_both_cases() {
        assert_eq!(Code::parse("E0001"), Some(Code::AssignToImmutable));
        assert_eq!(Code::parse("e0001"), Some(Code::AssignToImmutable));
        assert_eq!(Code::parse("E9999"), None);
    }

    /// The ends of the list and a range inside it, and neighbours outside it.
    #[test]
    fn format_characters_are_category_cf() {
        for c in [
            '\u{AD}',
            '\u{200B}',
            '\u{200F}',
            '\u{202E}',
            '\u{FEFF}',
            '\u{E0001}',
            '\u{E007F}',
        ] {
            assert!(is_format(c), "{c:?}");
        }
        for c in [
            'a',
            '\u{AC}',
            '\u{AE}',
            '\u{2010}',
            '\u{E0002}',
            '\u{E0080}',
            '\u{3042}',
        ] {
            assert!(!is_format(c), "{c:?}");
        }
    }

    #[test]
    fn codes_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for &code in Code::ALL {
            let text = code.as_str();
            let (series, digits) = text.split_at(1);
            assert!(
                matches!(series, "E" | "W")
                    && digits.len() == 4
                    && digits.bytes().all(|b| b.is_ascii_digit()),
                "{text}"
            );
            assert_eq!(Code::parse(text), Some(code));
            assert!(seen.insert(text), "{text} is used twice");
        }
    }
}
