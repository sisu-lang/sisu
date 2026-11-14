//! The registry: every diagnostic code, and the message, labels, help and fixes of each.

use rustc_hash::FxHashSet;

use super::{Diagnostic, Edit, Fix, Severity, Suggestion};
use crate::conformance::CHOICE_DEPTH_LIMIT;
use crate::payload_limit::{self, PAYLOAD_LIMIT_GIB};
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
    AssignToImmutable | PushToImmutable = "E0001",
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
    UnexpectedCharacter | LogicalOperatorWritten = "E0010",
    Expected | JumpTakesBlock | LetTakesNoType | RangeBoundMissing = "E0011",
    Var = "E0013",
    FieldComma = "E0015",
    FieldLet = "E0016",
    MethodNeedsSelf = "E0017",
    MutSelf = "E0018",
    SelfNotFirst = "E0020",
    AssignToExpression | PushToTemporary = "E0021",
    RangeOutsideFor = "E0023",
    EqualityChain = "E0024",
    ComparisonDirection = "E0025",
    BuiltInName = "E0026",
    NeverConstructed = "E0027",
    NoMain = "E0028",
    MainSignature = "E0029",
    UnknownType = "E0030",
    BodyHasNoValue = "E0031",
    Mismatch | PatternMismatch | StringGetNeedsRange | ArrayGetTakesIndex = "E0032",
    BindingNotFound = "E0033",
    DefinedTwice | InheritedRequirement = "E0034",
    ForNeedsArray = "E0035",
    ReturnNeedsValue = "E0036",
    UnexpectedReturnValue = "E0037",
    JumpOutsideLoop = "E0038",
    AlreadyDeclared = "E0039",
    IntegerDoesNotFit = "E0040",
    SelfOutsideMethod = "E0041",
    NegatedU8 = "E0042",
    NoValue | BareBreak = "E0043",
    NeverNone = "E0044",
    IsTestsNone = "E0045",
    IsNeedsObjects | AnyMayNotHold = "E0046",
    CannotCompare = "E0047",
    CoalesceNotOptional = "E0048",
    SafeAccessNotOptional = "E0049",
    CannotInfer | CannotInferParam | CannotInferSome | CannotInferResult = "E0050",
    UnwrapNeedsOptional = "E0051",
    FunctionNotFound | NotAFunction = "E0052",
    ArgumentCount = "E0053",
    AlreadyConverted = "E0054",
    LabeledArgument = "E0055",
    MissingField = "E0056",
    MethodNotField = "E0057",
    FieldNotMethod = "E0058",
    PrintArgument = "E0059",
    PrintNever = "E0061",
    MayBeNone | ChainWriteOnStruct = "E0062",
    NoMember = "E0063",
    TooManyArguments = "E0064",
    ArgumentNeedsLabel = "E0065",
    FieldOutOfOrder = "E0066",
    SameObject = "E0067",
    CannotIndex | StringIndex = "E0068",
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
    TypeAsValue | FunctionAsValue | InterfaceAsValue | EnumAsValue | ParamAsValue = "E0105",
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
    TypeArguments = "E0137",
    TypeArgument = "E0138",
    ParamLacks = "E0139",
    InstantiationCycle | ConformanceTooDeep = "E0140",
    BreakValueNotLoop = "E0141",
    LabelClash = "E0142",
    LabelReused = "E0143",
    MissingLabel = "E0144",
    LabelNotOnLoop | UnnamedLabel = "E0145",
    InterfaceBody = "E0146",
    InterfaceAsType = "E0147",
    NotAnInterface = "E0148",
    SelfOutside = "E0149",
    InheritanceCycle = "E0150",
    InheritedArgument = "E0151",
    ConformanceBody = "E0152",
    ConformsTwice = "E0153",
    NotConform = "E0154",
    CannotExtend = "E0155",
    Ambiguous = "E0156",
    BoundTooWide = "E0157",
    ClassOnly = "E0158",
    NoOperator = "E0159",
    PayloadTooLarge = "E0160",
    AliasCycle = "E0161",
    AliasTooLarge = "E0162",
    TooManyInstances = "E0163",
    CannotBeAny = "E0164",
    AnyForm = "E0165",
    MutMarkerMissing = "E0166",
    MutMarkerNotMut = "E0167",
    MutNotArgument = "E0168",
    MutArgumentNotPlace = "E0169",
    MutArgumentType = "E0170",
    MutPlaceTwice = "E0171",
    LetElseCanFinish = "E0172",
    LetElseValueEndsInBrace = "E0173",
    RefutablePattern = "E0174",
    StructFieldMut = "E0175",
    NanComparison = "W0005",
    UnreachableArm = "W0006",
    RedundantSemicolon = "W0007",
    UnusedLabel = "W0008",
    WhileTrue = "W0009",
    UnconditionalRecursion = "W0010",
    UnchangedLoopCondition = "W0011",
    UnusedItem | UnusedBlock = "W0012",
    UnusedMember = "W0013",
    UnusedVariable = "W0014",
    PrintsOptional = "W0015",
    WriteToCopy = "W0016",
}

/// Codes the compiler no longer reports, and their explanations (`docs/diagnostics/README.md`,
/// "Numbers"). `sisu explain` still prints them.
pub(crate) const RETIRED: &[(&str, &str)] = &[
    (
        "E0012",
        include_str!("../../../../docs/diagnostics/E0012.md"),
    ),
    (
        "E0014",
        include_str!("../../../../docs/diagnostics/E0014.md"),
    ),
    (
        "E0019",
        include_str!("../../../../docs/diagnostics/E0019.md"),
    ),
    (
        "E0022",
        include_str!("../../../../docs/diagnostics/E0022.md"),
    ),
    (
        "E0060",
        include_str!("../../../../docs/diagnostics/E0060.md"),
    ),
];

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
    /// An assignment at `at` to `name`, or to a part of it, which cannot change. `written` says
    /// which part.
    AssignToImmutable {
        name: String,
        at: Span,
        written: Written,
        cause: Immutable,
    },
    /// A `push` at `at` onto `name`, or onto an element of it, which cannot change.
    PushToImmutable {
        name: String,
        at: Span,
        cause: Immutable,
    },
    /// A statement at `at` after one that never finishes.
    UnreachableCode { at: Span },
    /// The binding `name`, at `at`, which is a `let mut` or a `mut` parameter, and which nothing
    /// reassigns.
    NeverReassigned {
        name: String,
        at: Span,
        form: NeverReassignedForm,
    },
    /// A binary `-` at `at` that starts a line.
    LeadingBinaryMinus { at: Span },
    /// The `bracket` at `at`, which nothing closes.
    UnclosedBracket { bracket: char, at: Span },
    /// A `{` at `at` that starts a line; `join`, when the line above can take it, moves it up.
    BraceOnNewLine { at: Span, join: Option<Join> },
    /// A `[` at `at` that starts a line inside brackets, after an expression.
    BracketOnNewLine { at: Span },
    /// An `else` at `at` that starts a line; `of` is what the `else` should follow, and `join`,
    /// when the line above can take it, moves it up.
    ElseOnNewLine {
        at: Span,
        of: ElseOf,
        join: Option<Join>,
    },
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
    /// false when the lower case literal may still be rejected, as a bare 2^63 is. It is `None`
    /// after a `.` or `?.`, where the parser takes only a name, so no change of the prefix gives
    /// a clean program.
    BasePrefixUpperCase {
        prefix: char,
        safe: Option<bool>,
        at: Span,
    },
    /// The base prefix at `at` (`0` and `prefix`) with no digits after it.
    BasePrefixWithoutDigits { prefix: char, at: Span },
    /// The `digit`, at `at`, in a literal of base `radix` (2, 8 or 16), which is not a digit there.
    DigitOutsideBase { digit: char, radix: u32, at: Span },
    /// The character `c` at `at`, which Sisu does not use.
    UnexpectedCharacter { c: char, at: Span },
    /// The `&` or `|` `c` at `at`, between two operands: logical and and logical or are `&&` and
    /// `||`.
    LogicalOperatorWritten { c: char, at: Span },
    /// The token `found` at `at`, where `what` should stand.
    Expected {
        what: String,
        found: String,
        at: Span,
    },
    /// The `return` or `break` `keyword` at `at`, at the end of the header of an `if`, a `while`,
    /// a `for` or a `match`, which took the `{ ... }` of the body as its value. `grouped` is the
    /// header through the jump, for the help that puts it in `( )`; it is `None` for a `break`
    /// that the `( )` would leave in a loop header (E0082) or outside every loop (E0038).
    JumpTakesBlock {
        keyword: &'static str,
        at: Span,
        grouped: Option<String>,
    },
    /// The `: type` at `at` of a `let ... else`, or of a `let` whose pattern is not a name. The
    /// value gives the names their types. `with_else` is whether an `else` follows the value.
    /// After a name, any later-line `else` counts. After a pattern, a later-line `else` counts
    /// only when `join` is set. `join` is the gap before an `else` on a later line, when the
    /// fix can move that `else` after the value.
    LetTakesNoType {
        at: Span,
        with_else: bool,
        join: Option<Span>,
    },
    /// A `var` at `at`, on a field or on a binding.
    Var { at: Span, field: bool },
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
    /// A `mut` at `keyword` in front of the `self`, in a method of `owner`; the fix removes
    /// `remove`, from the `mut` to the `self` or to a comment before it. In a class or an
    /// interface, removing it is `safe` when `self` is the whole parameter; in an enum it never
    /// is. `annotation` is the `: Type` after `self`, when the fix can remove it too.
    MutSelf {
        keyword: Span,
        remove: Span,
        annotation: Option<Span>,
        safe: bool,
        owner: Owner,
    },
    /// A body or a field at `at` in an interface, which holds signatures only.
    InterfaceBody { at: Span, form: InterfaceBodyForm },
    /// The interface `name`, written as `written` at `at`, where a type belongs. `written` is
    /// `None` for the target of an `extend` block, where a type parameter is no advice.
    InterfaceAsType {
        name: String,
        written: Option<String>,
        at: Span,
        /// Whether the interface can be `any`, which the help then offers.
        any: bool,
        /// How many `?` follow the name, which the `any` the help offers must keep.
        layers: usize,
        /// Whether the name is in the body of a type alias, which takes no bound: the help then
        /// offers `any` only.
        alias: bool,
    },
    /// The type `written` at `at`, which is a `kind` and no interface, where a bound, a
    /// conformance or an inherited interface belongs.
    NotAnInterface {
        written: String,
        at: Span,
        kind: NotInterface,
    },
    /// A `Self` at `at`, outside a class, an enum, an interface and an `extend` block.
    SelfOutside { at: Span },
    /// The interface `name` inherits from itself, `through` the interfaces in between; `at` is
    /// its inheritance list.
    InheritanceCycle {
        name: String,
        at: Span,
        through: Vec<String>,
    },
    /// The type alias `name`, at `at`, which refers to itself, `through` the aliases in between.
    /// `steps` holds each alias of the cycle in order, the alias that it uses, and where it writes
    /// that alias.
    AliasCycle {
        name: String,
        at: Span,
        through: Vec<String>,
        steps: Vec<(String, String, Span)>,
    },
    /// The written type `ty`, at `at`, which names more than `limit` distinct types once the type
    /// aliases in it are replaced.
    AliasTooLarge {
        ty: String,
        at: Span,
        limit: usize,
        /// Whether `limit` is the whole check's, not the written type's.
        check: bool,
    },
    /// `any` at `at`, written in a way that it does not take.
    AnyForm { at: Span, form: AnyFormKind },
    /// The argument at `at` of a call, whose parameter `name` is `mut`, and that has no `mut`.
    /// `declared` is where the parameter is declared.
    MutMarkerMissing {
        at: Span,
        name: String,
        callee: String,
        declared: Option<Span>,
        fix: MissingFix,
    },
    /// The `mut` at `at` before an argument whose parameter is not `mut`. `fix_end` is the end of
    /// the spaces and tabs after the keyword, for the fix that removes them with it.
    MutMarkerNotMut {
        at: Span,
        fix_end: usize,
        form: NotMutForm,
    },
    /// A `mut` at `at` before an expression that is no argument. `binding` is whether the
    /// expression is the value of a `let` that has no `mut`. `fix_end` is the end of the spaces
    /// and tabs after the keyword, for the fix that removes them with it.
    MutNotArgument {
        at: Span,
        binding: bool,
        fix_end: usize,
    },
    /// The argument at `at`, passed `mut`, that `cause` says is no place. `name` is the root
    /// binding or the field that `cause` is about.
    MutArgumentNotPlace {
        at: Span,
        name: String,
        cause: NotPlace,
    },
    /// The `mut` argument at `at`, written `arg`, whose type `found` is not the type `want` of
    /// the parameter. `name` is the parameter's, and `position` its place, from 1, when it has
    /// no name; `declared` is where it is declared. `promotes` when the place is no optional
    /// and `want` is the optional of its type. `chain` when the place goes through a `?.`.
    MutArgumentType {
        at: Span,
        arg: String,
        name: Option<String>,
        position: usize,
        callee: String,
        found: String,
        want: String,
        declared: Option<Span>,
        /// The parameter's type as it is declared: it names a type parameter where `want` is
        /// the type that inference fixed.
        declared_as: String,
        promotes: bool,
        chain: Option<ChainForm>,
    },
    /// The second `mut` argument of a call, at `second`, that is the place of the first, at
    /// `first`, or holds it, or is held by it. `shown` is the text of the place that is inside
    /// the other, and `inside` the text of the other, for the second form.
    MutPlaceTwice {
        first: Span,
        second: Span,
        shown: String,
        inside: Option<String>,
    },
    /// The `else` block of a `let ... else`, whose `}` is at `at`, which can reach its end.
    LetElseCanFinish { at: Span },
    /// The value at `at` of a `let ... else`, which ends in `}`.
    LetElseValueEndsInBrace { at: Span },
    /// The pattern at `at`, written `shown`, of a `let` that has no `else`. It can fail to match,
    /// or, with `always`, it matches every value of an enum or an optional.
    RefutablePattern {
        at: Span,
        shown: String,
        always: bool,
    },
    /// A `mut` at `keyword` in front of a field of a struct; the fix removes `remove`, from the
    /// `mut` to the field's name or to a comment before it.
    StructFieldMut { keyword: Span, remove: Span },
    /// The interface `interface`, as written after `any` at `at`, which cannot be `any`. `owner`
    /// is the interface that has the `reason`: `interface` itself, or one it inherits.
    CannotBeAny {
        interface: String,
        owner: Option<String>,
        reason: NotAny,
        at: Span,
    },
    /// `mono` has made more than `limit` instances of the generic items of the program. `name` is
    /// the generic item with the most instances when `mono` stops, `count` of them. `at` is the
    /// name of that item when it is the user's, and else the user code that `mono` was lowering.
    TooManyInstances {
        name: String,
        at: Span,
        count: usize,
        limit: usize,
    },
    /// The argument `arg`, at `at`, of an inherited interface of `interface`, which is built from
    /// a type parameter.
    InheritedArgument {
        arg: String,
        interface: String,
        at: Span,
    },
    /// The requirement `name` of `interface`, at `at`, which `owner`, an interface that
    /// `interface` inherits, has at `first` already.
    InheritedRequirement {
        name: String,
        interface: String,
        owner: String,
        at: Span,
        first: Span,
        /// Whether `name` is an associated type, not a method.
        assoc: bool,
    },
    /// The conformance of `ty` to `interface`, whose name is at `at`, and what is wrong with it.
    ConformanceBody {
        ty: String,
        interface: String,
        at: Span,
        form: ConformanceForm,
    },
    /// A second conformance of `ty` to `interface`, whose name is at `at`; the first is at
    /// `first`. `generic` when the interface takes type parameters.
    ConformsTwice {
        ty: String,
        interface: String,
        at: Span,
        first: Span,
        generic: bool,
    },
    /// `ty` does not conform to `interface`, which `form` says why that is needed, at `at`.
    NotConform {
        ty: String,
        interface: String,
        at: Span,
        form: NotConformForm,
    },
    /// An `extend` of `written`, at `at`, which is no class or enum.
    CannotExtend {
        written: String,
        at: Span,
        form: CannotExtendForm,
    },
    /// The bound or conformance `written`, written at `at`, with the interfaces it inherits gives
    /// more than `limit` interfaces.
    BoundTooWide {
        kind: WideKind,
        written: String,
        limit: usize,
        at: Span,
    },
    /// The conformance of the type `ty` to `AnyObject`, whose name is at `at`, though only a class
    /// conforms to it. `via` is the interface that inherits `AnyObject`, and where, when `ty`
    /// does not write the conformance itself.
    ClassOnly {
        ty: String,
        at: Span,
        via: Option<(String, Span)>,
    },
    /// The operator `op` on the operand at `at`, of the concrete type `ty`, which has none: the
    /// interface of the operator is missing from `ty`, or from the part `leaf` of it.
    NoOperator {
        ty: String,
        op: String,
        at: Span,
        /// The part of `ty` that has none, when it is not `ty` itself.
        leaf: Option<String>,
        form: NoOperatorForm,
        /// A conformance that the type has already, whose bound fails: the bound as written
        /// there, and where it is.
        needs: Option<(String, Span)>,
    },
    /// The enum `name`, declared at `at`, has a payload that can be 4 GiB or more
    /// (`payload_limit`). Or the struct `name` can be 4 GiB or more, and `kind` says which. A
    /// payload or a struct must be less than 4 GiB. That is the limit of the language for every
    /// alignment, and a value of that size can never be on the stack. `instance` is whether `name`
    /// is an instance of the generic enum declared at `at`. `holder` is the enum of the prelude
    /// that holds `name` inline, when its payload is the one of 4 GiB or more. `used` is the first
    /// expression that uses the instance, when `mono` knows it.
    PayloadTooLarge {
        name: String,
        kind: payload_limit::Kind,
        instance: bool,
        holds: payload_limit::Holds,
        holder: Option<String>,
        at: Span,
        used: Option<Span>,
    },
    /// A call of the method `method` of `receiver`, at `at`, that more than one conformance
    /// could answer: those of `interfaces`. `annotate` is the `let` that picks one.
    Ambiguous {
        method: String,
        receiver: String,
        interfaces: Vec<String>,
        at: Span,
        annotate: Option<Annotate>,
    },
    /// A `self` at `at` that is not a method's first parameter.
    SelfNotFirst { at: Span },
    /// An assignment at `at` to something that is not a place. `range` is the range index that
    /// the target holds, at `at`: the checker, which knows the type of its base, reports it.
    AssignToExpression {
        at: Span,
        range: Option<RangeTarget>,
    },
    /// A `push` at `at` onto something that is not a place; `range` says which range index it
    /// holds, if any.
    PushToTemporary { at: Span, range: Option<RangePush> },
    /// A range at `at` where it is not allowed, in the way that `form` says.
    RangeOutsideFor { at: Span, form: RangeForm },
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
    /// A `main` at `at` that takes anything but nothing or one `[string]`, or returns a value;
    /// or, by `form`, that takes type parameters.
    MainSignature { at: Span, form: MainForm },
    /// The type `name`, at `at`, which is not built in and names no class; or, by `form`, an
    /// associated type that cannot be written there.
    UnknownType {
        name: String,
        at: Span,
        form: UnknownForm,
    },
    /// The function `function`, whose body, ending at `at`, has no value of type `want`.
    BodyHasNoValue {
        function: String,
        want: String,
        at: Span,
    },
    /// A value of type `got`, at `at`, where `want` is expected; `fix` is the
    /// rewrite that would fit. `origin` is where the expected type came from, if the report
    /// points at it. `conversion` is whether the value is a conversion that can fail, whose
    /// optional stands where its payload is expected. `layers` is whether `got` is `want` with
    /// two or more `?` layers taken off: a value converts through one `?` only.
    Mismatch {
        want: Expected,
        got: String,
        at: Span,
        origin: Option<MismatchOrigin>,
        fix: Option<MismatchFix>,
        conversion: bool,
        layers: bool,
    },
    /// The binding `name`, at `at`, which no scope holds.
    BindingNotFound {
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
        /// The start of the `break` whose label fix must wrap the jump in parentheses, as the
        /// jump ends a head.
        wrap: Option<usize>,
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
    /// The first bare `break` at `at` of a `loop` whose first `break` with a value whose type is
    /// not `never` is at `valued`.
    BareBreak { at: Span, valued: Span },
    /// A comparison at `at` of a value that is not optional with `None`. `value` is that value's
    /// path as written, when it has one.
    NeverNone { value: Option<String>, at: Span },
    /// An `is` at `at` with `None` on one side. `tested`, when `== None` would be right, is the
    /// path of the other side as written.
    IsTestsNone { tested: Option<String>, at: Span },
    /// An `is` whose operand at `at`, of type `found`, is no object. `help` when `==` may
    /// compare it: its type names no type parameter. `add`, for a type parameter in scope, is
    /// the bound that makes it an object. `peel`, for an optional of an optional, is the `is`
    /// with the extra layers of each operand taken off, as written.
    IsNeedsObjects {
        found: String,
        at: Span,
        help: bool,
        add: Option<AddBound>,
        peel: Option<PeelIs>,
    },
    /// An `is` whose operand at `at`, of the type `found`, which is an `any` type, may hold no
    /// object. `make` is the interface of the program that would make it so, when it is one.
    AnyMayNotHold {
        found: String,
        at: Span,
        make: Option<String>,
    },
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
    CannotInfer {
        literal: Literal,
        /// What the `let` that the literal initializes writes before its type, for the example.
        binding: Option<String>,
        /// The whole type for the example (`[char]` or `string?`), when a later use of the
        /// local gives one. Without it, the example writes `...` for the type.
        ty: Option<String>,
        at: Span,
        /// For the `None` that starts the right side of a `??`, the offset where the left side
        /// of that `??` starts: `??` groups to the right, and parentheses from there to `at`
        /// group it to the left.
        regroup: Option<usize>,
    },
    /// A call at `at` of `callee` that binds nothing to its type parameter `param`; `example`
    /// writes the call with its type arguments.
    CannotInferParam {
        param: String,
        callee: String,
        example: String,
        at: Span,
        /// Whether the callee's result type names `param`, which annotating the result then
        /// fixes.
        in_result: bool,
    },
    /// A call of `Some`, or of `Option.Some`, written as `callee` at `at`, whose argument does
    /// not fix the type of its payload.
    CannotInferSome { callee: String, at: Span },
    /// A call of `Ok` or `Err`, named `variant`, at `at`. Its arguments and context do not fix
    /// the type parameter `param` of its `Result`.
    CannotInferResult {
        param: String,
        variant: String,
        at: Span,
    },
    /// The value at `at`, of type `found`, that `keyword` unwraps, and which is not optional.
    UnwrapNeedsOptional {
        keyword: Unwrap,
        found: String,
        at: Span,
        /// Whether the pattern is a literal of the type that `found` names, so that `==` compares
        /// the value with it.
        compare: bool,
    },
    /// A call at `at` of `name`, which names no function.
    FunctionNotFound {
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
    },
    /// A call at `at` of `name`, which is `what` and no function: a binding, a constant, a type
    /// parameter or an associated type.
    NotAFunction {
        name: String,
        what: &'static str,
        at: Span,
    },
    /// A call at `at` of `name`, which takes `want` arguments, with `found`.
    ArgumentCount {
        name: String,
        want: usize,
        found: usize,
        at: Span,
    },
    /// A conversion to `ty` of the value at `at`, which already has that type. `value` is its
    /// path as written, when it has one, and `conversion` is the whole call, with any parentheses
    /// around it. `grouped` is set when the value is more than one term, or a comment lies in the
    /// head or the tail of the call. Then removing the conversion keeps its parentheses.
    /// `labeled` is set when the argument has a label, which the removal would delete along with
    /// its own error, so there is no fix. `head` is the text from the type's name to `at`, in the
    /// parts between its comments, which a grouped removal keeps.
    AlreadyConverted {
        value: Option<String>,
        ty: String,
        at: Span,
        conversion: Span,
        grouped: bool,
        labeled: bool,
        head: Vec<Span>,
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
    /// A call of `function` (`print` or `eprint`) at `at` with no argument or several, or one that
    /// returns nothing.
    PrintArgument {
        function: &'static str,
        problem: PrintProblem,
        at: Span,
    },
    /// The argument at `at` of a call of `function`, which never finishes.
    PrintNever { function: &'static str, at: Span },
    /// The optional at `at` that a member, an index or a `for` uses as a value. `value` is its
    /// path as written, when it has one. `access` is the `.` before the member, and any space or
    /// comment around it, when a `?.` can take its place. `write` when the member is the target of
    /// an assignment or a `push`: then the `?.` makes the statement do nothing, and has no result.
    MayBeNone {
        value: Option<String>,
        at: Span,
        access: Option<Span>,
        write: bool,
        /// For a `?.` on an optional of an optional: the optional it leaves, which has no members.
        peel: Option<Peel>,
        /// What the help says to unwrap.
        unwrap: UnwrapHelp,
    },
    /// A write at `at`, from the end of a `?.`'s base to its field, that the `?.` on a struct
    /// value leads to. `replace` is the base as written, when the help can name it.
    ChainWriteOnStruct { at: Span, replace: Option<String> },
    /// The `kind` member `name`, at `at`, which `owner` does not have.
    NoMember {
        owner: String,
        kind: Definition,
        name: String,
        at: Span,
        suggestion: Option<Suggestion>,
        /// For a type parameter: the bound that gives the method.
        add: Option<AddBound>,
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
    /// The value at `at` of an array fill, an object of `class` or an optional of one; with
    /// `param`, `class` is a type parameter, which may be a class, or an `any` type of an
    /// interface that is not class-only, which may hold an object. With `any`, `class` is an
    /// `any` type.
    SameObject {
        class: String,
        at: Span,
        param: bool,
        any: bool,
    },
    /// An index into the value at `at`, of type `ty`, which is no array; with `range`, a range
    /// index into one that is neither an array nor a string.
    CannotIndex { ty: String, at: Span, range: bool },
    /// A single index into the string at `at`: a string takes a range index only. `base` is the
    /// string as written on one line, or `None` when it has no such text. `usage` says whether the
    /// index reads or is the target of a write.
    StringIndex {
        at: Span,
        base: Option<String>,
        usage: IndexUsage,
    },
    /// The argument at `at` of a string's `get`, which is no range written in the call: `base` is
    /// the receiver as written on one line, or `None` when it has no such text, and `call` how the
    /// call is written.
    StringGetNeedsRange {
        at: Span,
        base: Option<String>,
        call: GetCall,
    },
    /// The range at `at`, given to an array's `get`, which takes an `i64`. `base` is the receiver
    /// as written, when `base[a..b]` is the help: the receiver is an array and the call is `.`.
    ArrayGetTakesIndex { at: Span, base: Option<String> },
    /// The method `method` at `at`, called on an array of type `ty`. The prelude gives the method
    /// to the array types `targets`, and none of them is `ty`. `function` is whether the call
    /// writes the method as a function, `method(xs)`, and the label then says it is a method: it
    /// states the cause, so it is not a help.
    SumNeedsNumbers {
        method: String,
        ty: String,
        targets: Vec<String>,
        at: Span,
        function: bool,
    },
    /// A call at `at` of the method `name` of an array, a number, a class or an enum as a
    /// function, with its `receiver`.
    MethodAsFunction {
        name: String,
        at: Span,
        receiver: MethodReceiver,
        /// A bound of the method's `extend` block that the receiver's type does not meet: the
        /// method call is an error too, so the report has no fix.
        bound: Option<BlockBound>,
    },
    /// The token at `at`, where the tree passes `limit` levels, and what made it deep.
    NestingTooDeep {
        at: Span,
        limit: usize,
        form: NestingForm,
    },
    /// The first byte at `at` that is not UTF-8, which the text shown in its place replaces.
    InvalidUtf8 { at: Span },
    /// An assignment to the `self` at `at`, the `self` of a struct method when `struct_self`.
    AssignToSelf { at: Span, struct_self: bool },
    /// An assignment to `name`, at `at`, which is a function.
    AssignToFunction { name: String, at: Span },
    /// A `let` that binds the `_` at `at`. The fix removes `remove`: the `mut` and the white
    /// space after it, up to a comment.
    MutOnUnderscore { at: Span, remove: Span },
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
    /// The interpolated value at `at`, of type `ty`, which interpolation cannot write as text.
    CannotInterpolate { ty: String, at: Span },
    /// The printed or interpolated optional at `at`, which may print `None`. `value` is its path
    /// as written, when it has one.
    PrintsOptional { value: Option<String>, at: Span },
    /// The write at `at` to the local declared at `copy`, a copy of the place `place`, which
    /// nothing reads; `help` names what the root of the place allows. A `place` that spans lines
    /// or holds a comment is `None`, and the diagnostic names only the `root`.
    WriteToCopy {
        place: Option<String>,
        root: String,
        at: Span,
        copy: Span,
        help: String,
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
    /// The built-in type or class `name`, at `at`, read as a value. A type alias `name` that
    /// names the type `alias` is read as one that has none: it has no constructor.
    TypeAsValue {
        name: String,
        at: Span,
        alias: Option<String>,
    },
    /// The interface `name`, at `at`, read as a value.
    InterfaceAsValue { name: String, at: Span },
    /// The type parameter or associated type `name`, at `at`, read as a value. `what` says which.
    ParamAsValue {
        name: String,
        what: &'static str,
        at: Span,
    },
    /// The enum `name`, at `at`, called as a function. `variant` is the first variant of the
    /// enum, written with the enum, for the help.
    EnumAsValue {
        name: String,
        variant: Option<String>,
        at: Span,
    },
    /// The function `name`, at `at`, read as a value. `takes_arguments` when it has parameters.
    FunctionAsValue {
        name: String,
        at: Span,
        takes_arguments: bool,
    },
    /// The member of `math`, `Math` or `f64`, written at `written`, which is not how Sisu writes
    /// math: a method or constant of another language, or `f64.max(a, b)` for `a.max(b)`; `fix`
    /// is the form Sisu uses, when it surely checks.
    MathIdiom { written: Span, fix: Option<MathFix> },
    /// The `**` at `at`, which Sisu has no operator for; `fix` is the `pow` call that may mean it.
    PowerOperator { at: Span, fix: Option<PowerFix> },
    /// The format spec at `at`, its `:` included, which the grammar `#` or
    /// `[<^>][width][.precision]` does not hold: it is empty, an alignment alone, or has a `.`
    /// without digits.
    MalformedFormatSpec { at: Span },
    /// The format spec at `at`, its `:` included, which asks for more than `#`, or alignment,
    /// width and precision: a leading `0`, a sign, a type, a fill, a space or a nested `{`. `form`
    /// says which of these forms it is.
    UnsupportedFormatSpec { at: Span, form: SpecForm },
    /// The format spec at `at`, its `:` included, whose width, when `width`, or else precision is
    /// above its limit.
    FormatSpecLimit { at: Span, width: bool },
    /// The precision of the spec at `at` on a part of type `ty`, which is not `f64`; `help` says
    /// what can take the precision, when something can.
    PrecisionNeedsF64 {
        ty: String,
        at: Span,
        help: Option<PrecisionHelp>,
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
    /// A local, parameter or binding at `at` named `name`, which names an enum, or an optional
    /// type through an alias.
    EnumNamedLocal {
        name: String,
        at: Span,
        /// Whether `name` is an alias of an optional type, which no local can be named like either.
        optional: bool,
    },
    /// The variant `name`, at `at`, written without its enum where no item or binding takes the
    /// name; `enums` are the enums that have such a variant, in declaration order. `labelled`
    /// are those of `enums` whose name is an open loop label that `at` would follow as the
    /// start of an unlabeled `break`'s value: they get no fix.
    BareVariant {
        name: String,
        enums: Vec<String>,
        labelled: Vec<String>,
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
    /// The member `name`, at `at`, of the type `type_name`, used as the other kind of member: a
    /// `method` named through its struct or enum, or else a variant called on a value. A method
    /// that a call names through its type has `receiver` when the first argument is a value of the
    /// type: the argument and whether the method call takes the other arguments. The fix calls
    /// the method on that value.
    MemberKind {
        name: String,
        type_name: String,
        method: bool,
        at: Span,
        receiver: Option<TypeReceiver>,
    },
    /// The field at `at`, read or assigned on a value of the enum `enum_name`.
    EnumField { enum_name: String, at: Span },
    /// An idiom of another language for an enum or `match`, at `at`.
    MatchIdiom { form: MatchIdiomForm, at: Span },
    /// A pattern written another language's way, from `at`.
    PatternForm { form: ForeignPattern, at: Span },
    /// The scrutinee at `at`, of type `ty`, which `match` cannot take.
    CannotMatch { ty: String, at: Span },
    /// The upper-case name `name`, at `at`, in a pattern where no enum is expected. `lower` is the
    /// name with its first letter in lower case, and what that spells: a binding, which the fix
    /// writes, or a keyword or a reserved word, which no binding can be.
    NotVariantHere {
        name: String,
        lower: LowerName,
        at: Span,
    },
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
    /// The fix writes an arm for `missing`, indented by `indent`, in place of `insert`: an empty
    /// span, or the space between the last arm and a `}` on its line. The arm is `_ =>` when
    /// `gap`, or when `missing` is `_`, an empty `match`: neither has a pattern of its own.
    /// `close` is the indentation of that `}`, which the fix moves to a line of its own.
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
    /// `remove` is the text that the fix deletes: the arm's line, or, when the `match`'s `}` ends
    /// that line, from the line break before the arm through its body; for an alternative, the
    /// alternative and the `|` that joins it to the one before it, or after it when it is first.
    /// It is one span, or, for an alternative inside brackets, the parts of that span between its
    /// comments, so that each comment stays.
    UnreachableArm {
        at: Span,
        alternative: bool,
        covered_by: Option<Span>,
        remove: Vec<Span>,
    },
    /// A `;` at `at` that separates nothing. `space` when it touches a token on each side, so
    /// that removing it would join them.
    RedundantSemicolon {
        at: Span,
        /// What the fix removes: the `;`, and the blanks after it at the start of a line. `None`
        /// means no fix.
        remove: Option<Span>,
        space: bool,
    },
    /// The pattern at `at` of `keyword`, which every value of its enum matches. `plain` says what
    /// a plain `let` does for it.
    AlwaysMatches {
        keyword: Unwrap,
        at: Span,
        plain: Plain,
    },
    /// A range written another language's way, reported at `at`: its operator, or the first
    /// negative bound of a colon slice that has one.
    ForeignRange { form: ForeignRange, at: Span },
    /// The range pattern at `at`, which has one bound or none (`..` alone). `start` and `end` are
    /// the bounds as written, and `None` for each one that is missing; `inclusive` is whether the
    /// operator is `..=`.
    RangeBoundMissing {
        start: Option<String>,
        inclusive: bool,
        end: Option<String>,
        at: Span,
    },
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
    /// The value at `at` of a `break` whose target, a loop of keyword `keyword` (`for`, `while`
    /// or `while let`) at `loop_at`, has no value.
    BreakValueNotLoop {
        keyword: &'static str,
        at: Span,
        loop_at: Span,
    },
    /// The loop label `name` at `at`, after `break` or `continue`, declared at `declared`, which
    /// `form` says reads as more than a loop label.
    LabelClash {
        name: String,
        at: Span,
        declared: Span,
        form: ClashForm,
    },
    /// The loop label `name` at `at`, which the enclosing loop whose loop label is at `outer`
    /// already takes.
    LabelReused { name: String, at: Span, outer: Span },
    /// The name `name` at `at` after `keyword` (`break` or `continue`), which names no open loop
    /// label; `suggestion` is the open loop label closest to it, which a `break` always has.
    MissingLabel {
        name: String,
        keyword: &'static str,
        at: Span,
        suggestion: Option<String>,
        /// The start of the `break` whose label fix must wrap the jump in parentheses, as the
        /// jump ends a head.
        wrap: Option<usize>,
    },
    /// The loop label `name:` at `at`, before the token at `next`, which starts no loop.
    LabelNotOnLoop { at: Span, next: Span },
    /// The `_:` at `at` before a loop; `remove` runs from it to the loop's keyword.
    UnnamedLabel { at: Span, remove: Span },
    /// The loop label `name` at `at`, which no jump names; `remove` runs from it to its loop's
    /// keyword.
    UnusedLabel {
        name: String,
        at: Span,
        remove: Span,
    },
    /// The `while` at `at`, from the keyword through its condition `true`, parentheses included;
    /// `fix` is `loop` in its place, unless a jump or loop label in the loop failed (a `break`
    /// with a value, E0141, for one), as the rewrite could remove that error.
    WhileTrue { at: Span, fix: bool },
    /// The function or method `name`, whose name in its declaration is at `at`, which calls
    /// itself on every path; `calls` are the self-calls that end a path, in source order.
    UnconditionalRecursion {
        kind: Definition,
        name: String,
        at: Span,
        calls: Vec<Span>,
    },
    /// The `while` condition at `at`, which nothing in the loop changes; `names` are the locals
    /// it reads and `lets` the `let` bindings among them, both in source order.
    UnchangedLoopCondition {
        at: Span,
        names: Vec<String>,
        lets: Vec<String>,
    },
    /// The function, method, class, enum or constant `name` (its `kind`), whose name in its
    /// declaration is at `at`, which no live code uses; `uses` are up to three uses in dead code,
    /// each with the kind and name of its dead user. `named` is, for a struct or an enum, the
    /// first place outside its declaration where a type names it.
    UnusedItem {
        kind: Definition,
        name: String,
        at: Span,
        named: Option<Span>,
        uses: Vec<(Span, UsedBy, String)>,
    },
    /// The `extend` block of the type written `name`, whose header is at `at`, which no live code
    /// uses, while live code builds its type; `uses` are as `UnusedItem`'s.
    UnusedBlock {
        name: String,
        at: Span,
        uses: Vec<(Span, UsedBy, String)>,
    },
    /// The field or variant `name` (its `kind`), whose name in its declaration is at `at`, which
    /// no live code reads or builds; `uses` are as `UnusedItem`'s. `shown` is the `dbg` call that
    /// shows a field, which is not a read.
    UnusedMember {
        kind: Definition,
        name: String,
        at: Span,
        shown: Option<Span>,
        uses: Vec<(Span, UsedBy, String)>,
    },
    /// The local `name`, or the parameter when `param`, declared at `at`, which nothing reads;
    /// `fix` holds, for a `for` or pattern name, each place that binds it, which `_` can replace.
    UnusedVariable {
        param: bool,
        name: String,
        at: Span,
        fix: Vec<Span>,
    },
    /// The comparison at `at`, `!=` when `negated`, one side of which is `f64.nan`, which no
    /// number equals; `fix` is its `is_nan` test, when the other side is an `f64`.
    NanComparison {
        at: Span,
        negated: bool,
        fix: Option<NanFix>,
    },
    /// Type arguments that `name` does not take as written; `form` says how.
    TypeArguments {
        name: String,
        at: Span,
        form: ArgumentsForm,
    },
    /// The type argument at `at`, `ty`, which no parameter may take: `unit` or `never`.
    TypeArgument { ty: String, at: Span },
    /// The operator `op` on the operand at `at`, whose type `ty` names a type parameter, which
    /// has no operators.
    ParamLacks {
        ty: String,
        op: String,
        at: Span,
        /// The bound that gives the operator, when the type parameter that lacks it is in scope.
        add: Option<AddBound>,
    },
    /// The generic declaration `name`, which instantiates itself without end: `instance`, at
    /// `at`, gives a growing type argument, and `through` holds each other instantiation on the
    /// way back to `name`, as written, at its site.
    InstantiationCycle {
        name: String,
        at: Span,
        instance: Spelled,
        through: Vec<(Span, Hop)>,
        /// The interface whose hub the cycle passes, and how the program spells the call through
        /// it: a call through it counts as reaching every conformance to it.
        reach: Option<(String, Spelled)>,
        /// The associated type, as written, whose choices the cycle passes: the check counts it as
        /// every choice that it cannot show to be impossible.
        choice: Option<String>,
        /// The type, as written, whose argument grows in the operand of a step that the program
        /// writes, before the edge out of a hub, and the type of the operand that holds it when
        /// that is another type.
        grows: Option<(String, Option<String>)>,
    },
    /// The check of `interface` for the type `root`, which stopped after `limit` nested types:
    /// the class or enum `holder`, named at `at`, is the one that the check was too deep for.
    ConformanceTooDeep {
        interface: String,
        root: String,
        limit: usize,
        holder: String,
        at: Span,
    },
}

/// What an instantiation on a cycle does, as a note at its site says.
pub(crate) enum Hop {
    /// It instantiates `instance`, as written.
    Instantiates(String),
    /// It calls through `interface`, which may run `method`, the one the cycle takes. `by` is the
    /// spelling in the program, when it is not a call of the method.
    Calls {
        by: Option<Spelled>,
        method: String,
        interface: String,
    },
}

/// An instantiation on a cycle, as the program spells it.
#[derive(Clone)]
pub(crate) enum Spelled {
    /// A function, a method or a type, or a call of a method of an interface, as written.
    Instance(String),
    /// A call of a function or a method of the prelude, by its own name.
    Prelude(String),
    /// An operator or a built-in function that calls through a language interface: `==`, `<`,
    /// `print` or `dbg`.
    Operator(String),
    /// A part of an interpolation, which calls through `Display`.
    Interpolation,
    /// The conversion of a value to the `any` type named, which calls through the interfaces of
    /// its table.
    Conversion(String),
    /// The generated `==` (or `to_string`, when `display`) of the enum `enum_name`, which uses a
    /// payload of type `operand`.
    Generated {
        display: bool,
        enum_name: String,
        operand: String,
    },
}

impl Spelled {
    /// The text of a label at the place of the instantiation.
    fn label(&self) -> String {
        match self {
            Spelled::Generated {
                display, operand, ..
            } => {
                let verb = if *display { "shows" } else { "compares" };
                format!("{} {verb} `{operand}` here", self.subject())
            }
            _ => format!("{} here", self.subject()),
        }
    }

    /// What the program spells at the place, as the subject of a sentence.
    fn subject(&self) -> String {
        match self {
            Spelled::Instance(name) | Spelled::Prelude(name) => format!("`{name}`"),
            Spelled::Operator(op) => format!("`{op}`"),
            Spelled::Interpolation => "the interpolation".to_string(),
            Spelled::Conversion(any) => format!("the conversion to `{any}`"),
            Spelled::Generated {
                display, enum_name, ..
            } => {
                let method = if *display { "to_string" } else { "==" };
                format!("the `{method}` of `{enum_name}`")
            }
        }
    }

    /// The help that says what the spelling is, when it is not a call of the method.
    fn call_through(&self, interface: &str) -> Option<String> {
        match self {
            Spelled::Operator(op) => Some(format!("`{op}` is a call through `{interface}`, and ")),
            Spelled::Interpolation => Some(format!(
                "an interpolation is a call through `{interface}`, and "
            )),
            Spelled::Prelude(name) => {
                Some(format!("`{name}` is a call through `{interface}`, and "))
            }
            Spelled::Conversion(any) => Some(format!(
                "the conversion to `{any}` is a call through `{interface}`, and "
            )),
            Spelled::Instance(_) | Spelled::Generated { .. } => None,
        }
    }
}

/// The receiver of a method that a call writes as a function.
pub(crate) enum MethodReceiver {
    /// The first argument. `safe` is whether the method call checks.
    Argument {
        argument: ReceiverArgument,
        safe: bool,
    },
    /// The first argument, an optional whose payload has the method. `value` is its path as
    /// written, when it has one, and `takes` is whether the method takes the other arguments.
    /// The fix is never safe: `?.` skips the call on `None`.
    Optional {
        argument: ReceiverArgument,
        value: Option<String>,
        takes: bool,
    },
    /// The `self` of the method that holds the call, which has the method. `safe` is whether
    /// the method call checks.
    SelfValue { safe: bool },
}

/// A first argument that becomes the receiver of the method call. `head` and `gap` are the text
/// that the fix deletes, as the parts between its comments (`Source::removal_parts`), so that each
/// comment stays.
pub(crate) struct ReceiverArgument {
    /// The text from the callee to the receiver.
    pub(crate) head: Vec<Span>,
    /// The text from the receiver to the next argument, if any, else the empty span at the end of
    /// the receiver.
    pub(crate) gap: Vec<Span>,
    /// Whether the receiver needs parentheses to take a `.name(...)`: it is no postfix
    /// expression, or a comment comes before it.
    pub(crate) wrap: bool,
}

impl ReceiverArgument {
    /// The edits that move the receiver before the method name. `access` replaces the first part
    /// of the gap: it ends with the `(` of the method call.
    fn edits(&self, access: &str) -> Vec<Edit> {
        let (open, close) = if self.wrap { ("(", ")") } else { ("", "") };
        let mut edits = part_edits(&self.head, open);
        edits.extend(part_edits(&self.gap, &format!("{close}{access}")));
        edits
    }
}

/// The first argument of a call through a type, which becomes the receiver of the method call.
/// `takes` is whether the method call takes the other arguments. `safe` is whether the fixed
/// program checks: the method call takes them and its result meets the context of the call.
pub(crate) struct TypeReceiver {
    pub(crate) argument: ReceiverArgument,
    pub(crate) takes: bool,
    pub(crate) safe: bool,
}

/// The message of the fix that calls the method `name` on the first argument of a call written as
/// a function. `takes` is whether the method call takes the other arguments: when it does not,
/// the message says that they need a fix.
fn method_form_message(name: &str, takes: bool) -> String {
    if takes {
        format!("call `{name}` as a method")
    } else {
        format!("try calling `{name}` as a method, then fix its arguments")
    }
}

/// How written type arguments do not fit what they follow.
pub(crate) enum ArgumentsForm {
    /// A generic declaration that takes `want` of them, with `found`.
    Count { want: usize, found: usize },
    /// A generic declaration, with none, whose parameters are `params`.
    Missing { params: Vec<String> },
    /// A declaration that takes none. The fix deletes the list, the diagnostic's span; it is
    /// unsafe when the list holds a comment.
    NotGeneric { safe: bool },
    /// A value, which is not generic. `compare`, when the list is followed by `(`, is the text
    /// the list starts with, the value through the first argument, which a comparison would put
    /// in parentheses; before a `.` it was no comparison.
    Value { compare: Option<String> },
    /// A variant of a generic enum, which takes its enum's: `instead` writes them on the enum.
    Variant { instead: Option<String> },
}

/// What is wrong with the argument of a `print` or `eprint` call.
pub(crate) enum PrintProblem {
    /// The call has no argument or several.
    Count,
    /// The argument has no value: it is a call that returns nothing.
    NoValue,
}

/// What made a tree pass the nesting limit.
pub(crate) enum NestingForm {
    /// Nested expressions, blocks, patterns or types.
    Tree,
    /// A chain of operators, calls, fields or indexes, such as `a + b + c`, or of prefix
    /// operators: each link is one level, and the links are more than half the levels.
    Chain,
    /// A chain of `else if`: each one is one level, and they are more than half the levels.
    ElseIf,
    /// A use of a type alias: the text is the type as written, which names more than the limit,
    /// and the tree's token is where it is written.
    Alias(String),
}

/// Why `main`'s signature is wrong.
pub(crate) enum MainForm {
    /// It takes something but nothing or one `[string]`, or returns a value.
    Signature,
    /// It takes type parameters. The fix deletes `remove`, the list, when nothing else in
    /// `main` names them.
    TypeParams { remove: Option<Span> },
}

/// How a loop label at a jump also reads as something else.
pub(crate) enum ClashForm {
    /// A name in scope of the same spelling, a `kind` declared at `declared` when it has a span.
    Name {
        kind: ClashKind,
        declared: Option<Span>,
    },
    /// A token at `next` that can only continue an expression.
    Operator { next: Span },
    /// A `(` at `bracket` that touches the loop label; `text` runs from it through its `)`.
    Call { bracket: Span, text: String },
    /// A `[` at `bracket` that touches the loop label; `text` runs from it through its `]`.
    Index { bracket: Span, text: String },
}

/// What a name that a loop label clashes with is.
#[derive(Clone, Copy)]
pub(crate) enum ClashKind {
    Binding,
    Function,
    Constant,
    Class,
    Struct,
    Enum,
    Interface,
    Alias,
    BuiltInType,
}

impl ClashKind {
    fn word(self) -> &'static str {
        match self {
            ClashKind::Binding => "binding",
            ClashKind::Function => "function",
            ClashKind::Constant => "constant",
            ClashKind::Class => "class",
            ClashKind::Struct => "struct",
            ClashKind::Enum => "enum",
            ClashKind::Interface => "interface",
            ClashKind::Alias => "type alias",
            ClashKind::BuiltInType => "built-in type",
        }
    }
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
    /// The method call `receiver.<name>(next...)` for `written(receiver, next...)`. `receiver`
    /// holds the text that the fix deletes, and whether the receiver needs parentheses to take
    /// the call. `form` is the call as it reads.
    Method {
        name: &'static str,
        form: String,
        receiver: ReceiverArgument,
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

/// The variant `path` (`Expr.Num`) whose payload declares no labels, and the spans of each
/// `label: ` of a call of it, which its fix deletes: two spans where a comment lies before the `:`.
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

/// The statement or expression that unwraps an optional.
#[derive(Clone, Copy)]
pub(crate) enum Unwrap {
    IfLet,
    WhileLet,
    LetElse,
    /// A `let` with a pattern and no `else`.
    Let,
}

impl Unwrap {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Unwrap::IfLet => "if let",
            Unwrap::WhileLet => "while let",
            Unwrap::LetElse => "let ... else",
            Unwrap::Let => "let",
        }
    }
}

/// What a plain `let` does for an always-matching pattern (E0131).
#[derive(Clone, Copy)]
pub(crate) enum Plain {
    /// The pattern takes the value apart: only a `match` does that.
    No,
    /// A name or `_` of an `if let`: a plain `let` is the fix, with no edit.
    Let,
    /// A name or `_` of a `let ... else`: the fix removes `span`, the text from the end of the
    /// value through the `}` of the `else` block. `safe` when it removes no other diagnostic.
    RemoveElse { span: Span, safe: bool },
}

/// `names` in backticks, with `last` before the last one: "`a`", "`a` or `b`", "`a`, `b` or `c`".
fn listed(names: &[String], last: &str) -> String {
    let (end, rest) = names
        .split_last()
        .expect("a list should hold at least one name");
    if rest.is_empty() {
        return format!("`{end}`");
    }
    let rest: Vec<_> = rest.iter().map(|name| format!("`{name}`")).collect();
    format!("{} {last} `{end}`", rest.join(", "))
}

/// The most entries of a cycle that a diagnostic shows. A cycle of one more entry shows all, as
/// "and 1 more" is no shorter than the entry.
const CYCLE_SHOWN: usize = 5;

/// The first `CYCLE_SHOWN` of the entries of a cycle and the number left out, when the cycle is
/// longer than `CYCLE_SHOWN + 1`; else all of them and none.
fn cycle_shown<T>(entries: &[T]) -> (&[T], usize) {
    if entries.len() <= CYCLE_SHOWN + 1 {
        return (entries, 0);
    }
    (&entries[..CYCLE_SHOWN], entries.len() - CYCLE_SHOWN)
}

/// The names of a cycle in backticks, as `listed` with "and" gives them, cut as `cycle_shown`
/// does: "`a`, `b`, `c`, `d`, `e` and 3 more".
fn cycle_listed(names: &[String]) -> String {
    let (shown, more) = cycle_shown(names);
    if more == 0 {
        return listed(names, "and");
    }
    let shown: Vec<_> = shown.iter().map(|name| format!("`{name}`")).collect();
    format!("{} and {more} more", shown.join(", "))
}

/// `types` in backticks, each after "an", with "or" before the last one: "an `[i64]`",
/// "an `[i64]` or an `[f64]`", "an `[i64]`, an `[f64]` or an `[u8]`".
fn one_of(types: &[String]) -> String {
    let types: Vec<_> = types.iter().map(|t| format!("an `{t}`")).collect();
    let (end, rest) = types
        .split_last()
        .expect("a list should hold at least one type");
    if rest.is_empty() {
        return end.clone();
    }
    format!("{} or {end}", rest.join(", "))
}

/// `n` with a comma before each group of three digits: `100000` is `100,000`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let first = digits.len() % 3;
    let groups = (first..digits.len()).step_by(3);
    let mut out = digits[..first].to_string();
    for at in groups {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&digits[at..at + 3]);
    }
    out
}

/// `n` instances, with the singular for 1: `1 instance`, `2 instances`.
fn instances(n: usize) -> String {
    let noun = if n == 1 { "instance" } else { "instances" };
    format!("{} {noun}", thousands(n))
}

/// What a name is defined as.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Definition {
    Function,
    Class,
    Struct,
    Field,
    Method,
    Binding,
    Type,
    Constant,
    Enum,
    Variant,
    LoopLabel,
    Interface,
    Alias,
}

impl Definition {
    /// The indefinite article before `word`.
    fn article(self) -> &'static str {
        match self {
            Definition::Enum | Definition::Interface => "an",
            _ => "a",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Definition::Function => "function",
            Definition::Class => "class",
            Definition::Struct => "struct",
            Definition::Field => "field",
            Definition::Method => "method",
            Definition::Binding => "binding",
            Definition::Type => "type",
            Definition::Constant => "constant",
            Definition::Enum => "enum",
            Definition::Variant => "variant",
            Definition::LoopLabel => "loop label",
            Definition::Interface => "interface",
            Definition::Alias => "type alias",
        }
    }

    /// The verb of "is never ..." for an unused item or member of this kind.
    fn unused_verb(self) -> &'static str {
        match self {
            Definition::Function | Definition::Method => "called",
            Definition::Class | Definition::Struct | Definition::Enum | Definition::Variant => {
                "built"
            }
            Definition::Field => "read",
            Definition::Constant
            | Definition::Binding
            | Definition::Type
            | Definition::LoopLabel
            | Definition::Interface
            | Definition::Alias => "used",
        }
    }
}

/// The W0012 or W0013 of the item or member `name`, whose name is at `at`. `shown` is the `dbg`
/// call that shows it. `named` is the first type that names a struct or an enum.
fn unused_name(
    code: Code,
    kind: Definition,
    name: &str,
    at: Span,
    shown: Option<Span>,
    named: Option<Span>,
    uses: Vec<(Span, UsedBy, String)>,
) -> Diagnostic {
    let verb = kind.unused_verb();
    // A variant cannot take a `_` name (E0113).
    let help = if kind == Definition::Variant {
        "delete it".to_string()
    } else if named.is_some() {
        format!(
            "delete it and each type that names it, directly or through an alias, or name it `_{name}` everywhere if it is kept on purpose"
        )
    } else {
        format!("delete it, or name it `_{name}` if it is kept on purpose")
    };
    let d = Diagnostic::coded(
        code,
        at,
        format!("{} `{name}` is never {verb}", kind.word()),
    )
    .with_label(format!("never {verb}"))
    .with_help(help);
    let d = match shown {
        Some(call) => d.with_secondary(
            call,
            format!("`dbg` can show `{name}` here, but `dbg` is not a read"),
        ),
        None => d,
    };
    let d = match named {
        Some(at) => d.with_secondary(at, "named here"),
        None => d,
    };
    cite_uses(d, uses)
}

/// `d` with a secondary label for each of `uses`, a use in dead code.
fn cite_uses(d: Diagnostic, uses: Vec<(Span, UsedBy, String)>) -> Diagnostic {
    uses.into_iter().fold(d, |d, (at, user, name)| {
        d.with_secondary(at, format!("used here, {}", user.described(&name)))
    })
}

/// The dead code that holds a use which a warning cites.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsedBy {
    /// A function, a method or a constant.
    Item(Definition),
    /// An `extend` block of a type; `built` is whether live code builds the type.
    Block { built: bool },
}

impl UsedBy {
    /// What the label of a use says of its holder, which is written `name`: `in `f`, which is
    /// never called`.
    fn described(self, name: &str) -> String {
        match self {
            UsedBy::Item(kind) => format!("in `{name}`, which is never {}", kind.unused_verb()),
            UsedBy::Block { built: true } => {
                format!("in the `extend` block of `{name}`, which is never used")
            }
            UsedBy::Block { built: false } => {
                format!("in the `extend` block of `{name}`, whose type is never built")
            }
        }
    }
}

/// What gives more inherited interfaces than the limit.
#[derive(Clone, Copy)]
pub(crate) enum WideKind {
    /// A bound of a type parameter: `T: Shape`.
    Bound,
    /// An interface in the list of a class or an enum, or in an `extend` block: `K: Shape`.
    Conformance,
}

impl WideKind {
    fn noun(self) -> &'static str {
        match self {
            WideKind::Bound => "bound",
            WideKind::Conformance => "conformance",
        }
    }
}

/// What a name is that a bound, a conformance or an inherited interface names, and that is no
/// interface.
#[derive(Clone, Copy)]
pub(crate) enum NotInterface {
    Class,
    Struct,
    Enum,
    BuiltIn,
    TypeParam,
    Optional,
    Array,
    /// `any I`, an interface value.
    Any,
}

impl NotInterface {
    /// The kind with its article: `a class`.
    fn named(self) -> &'static str {
        match self {
            NotInterface::Class => "a class",
            NotInterface::Struct => "a struct",
            NotInterface::Enum => "an enum",
            NotInterface::BuiltIn => "a built-in type",
            NotInterface::TypeParam => "a type parameter",
            NotInterface::Optional => "an optional type",
            NotInterface::Array => "an array type",
            NotInterface::Any => "an interface value",
        }
    }
}

/// Why a type is unknown.
#[derive(Clone)]
pub(crate) enum UnknownForm {
    /// The name is no type; `suggestion` is a name in scope that it may have meant.
    Name { suggestion: Option<Suggestion> },
    /// The type parameter or associated type `base` has no associated type of the diagnostic's
    /// name. `add` is the bound that gives it one, when one interface declares it.
    NoAssociated { base: String, add: Option<AddBound> },
    /// The body of a type alias writes the associated type of an associated type, as `T.Item.Item`:
    /// `written` is that type and `base` the associated type it is taken from.
    OfAssociatedInAlias { written: String, base: String },
    /// The associated type is written through a class, an enum or another type that is no type
    /// parameter. `own` when the base is `Self` in a conformance, which writes the name alone.
    ConcreteBase { own: bool },
    /// The choice of an associated type names one of its own conformance. `shadows` when a class,
    /// an enum or a built-in type has the name too, and the associated type hides it.
    ChoiceNamesItself { shadows: bool },
    /// The choice of an associated type needs the associated type itself, which the diagnostic's
    /// name gives with its base, as `W<Leaf>.A`: the types the choice names lead back to it.
    ChoiceLoops,
    /// The choice of an associated type leads through more than `CHOICE_DEPTH_LIMIT` other
    /// associated types, one after the other, before it gives a type. The diagnostic's name gives
    /// the associated type that starts the chain.
    ChoiceTooDeep,
    /// The associated type of the type parameter `base` is written in a bound that the bounds of
    /// `base` wait for, so the bounds of `base` are not known yet.
    BoundsLoop { base: String },
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

/// What a mismatch expected.
pub(crate) enum Expected {
    /// One of these types.
    Types(Vec<String>),
    /// Any array, as the outer shape of a type that still names an unbound parameter.
    Array,
    /// Any class or enum with this name, as `Box`, for the same reason.
    Named(String),
}

impl Expected {
    /// The text of the expectation: the types in backticks, or the shape in words.
    fn text(&self) -> String {
        match self {
            Self::Types(types) => listed(types, "or"),
            Self::Array => "an array".to_string(),
            Self::Named(name) => format!("a `{name}`"),
        }
    }
}

/// Where a mismatch's expected type came from: the fact that the report names.
#[derive(Clone)]
pub(crate) enum MismatchOrigin {
    /// The `string` operand of a `+` that expects a `string` of the other.
    Concat(Span),
    /// The earlier alternative's binding of `name`, at `at`, which gave it the type wanted.
    Alternative { name: String, at: Span },
    /// The `break` at `at`, the first of its `loop` with a value, gave the loop the type `ty`.
    Break { at: Span, ty: String },
    /// `function` declares its parameter `param` as `ty`, written at `at`.
    Parameter {
        at: Span,
        function: String,
        param: String,
        ty: String,
    },
    /// `function` returns `ty`, written at `at`.
    Returns {
        at: Span,
        function: String,
        ty: String,
    },
    /// A `let` or a constant declares the type `ty` at `at`.
    Declared { at: Span, ty: String },
    /// The class declares its field `field` as `ty`, written at `at`.
    Field { at: Span, field: String, ty: String },
    /// The payload of `variant` holds `ty`, written at `at`.
    Payload {
        at: Span,
        variant: String,
        ty: String,
    },
    /// The local `name`, declared at `at`, has the type `ty`.
    Assigned { at: Span, name: String, ty: String },
    /// The rule that a condition is a `bool`.
    Condition,
    /// The rule that an index of an array value is an `i64` or a range.
    Index,
    /// The rule that an index of a place is an `i64`.
    PlaceIndex,
    /// The element at `at` gave its inferred array the type `ty`.
    Element { at: Span, ty: String },
    /// The branch value at `at` gave its `if` or `match` the type `ty`.
    Branch { at: Span, ty: String },
    /// The argument at `at` gave the type parameter `param` the type `ty`.
    Inferred { at: Span, param: String, ty: String },
}

impl MismatchOrigin {
    /// Where the fact points, and its text: no span for a language rule.
    fn text(self, want: &str) -> (Option<Span>, String) {
        match self {
            Self::Concat(operand) => (Some(operand), "`+` joins this `string`".to_string()),
            Self::Alternative { name, at } => (Some(at), format!("`{name}` is {want} here")),
            Self::Break { at, ty } => (
                Some(at),
                format!("this `break` gave the loop the type `{ty}`"),
            ),
            Self::Parameter {
                at,
                function,
                param,
                ty,
            } => (
                Some(at),
                format!("`{function}` declares `{param}` as `{ty}`"),
            ),
            Self::Returns { at, function, ty } => {
                (Some(at), format!("`{function}` returns `{ty}`"))
            }
            Self::Declared { at, ty } => (Some(at), format!("declared as `{ty}` here")),
            Self::Field { at, field, ty } => (Some(at), format!("`{field}` is declared as `{ty}`")),
            Self::Payload { at, variant, ty } => (Some(at), format!("`{variant}` holds `{ty}`")),
            Self::Assigned { at, name, ty } => (Some(at), format!("`{name}` has the type `{ty}`")),
            Self::Condition => (None, "a condition must be `bool`".to_string()),
            Self::Index => (None, "an index must be `i64` or a range".to_string()),
            Self::PlaceIndex => (None, "an index must be `i64`".to_string()),
            Self::Element { at, ty } => (Some(at), format!("this element gives the type `{ty}`")),
            Self::Branch { at, ty } => (Some(at), format!("this branch gives the type `{ty}`")),
            Self::Inferred { at, param, ty } => (
                Some(at),
                format!("this argument gives `{param}` the type `{ty}`"),
            ),
        }
    }
}

/// The form of an `UnsupportedFormatSpec`, which sets its message and help.
#[derive(Clone, Copy)]
pub(crate) enum SpecForm {
    /// Any spec that the other forms do not name.
    Plain,
    /// A `#` beside an alignment, a width or a precision, and which of the two limits the rest
    /// of the spec passes, if it does.
    Pretty(Option<SpecLimit>),
    /// `?` alone, which Rust uses for the debug form.
    Debug,
    /// A `?` after a spec that is valid without it, such as `>5?`.
    DebugSpec,
    /// `#?` alone, which Rust uses for the debug form over lines.
    PrettyDebug,
}

/// The limits that a spec passes: a width over 1000, a precision over 30, or both.
#[derive(Clone, Copy)]
pub(crate) enum SpecLimit {
    Width,
    Precision,
    Both,
}

/// What helps a precision on a part that is not an `f64`.
#[derive(Clone)]
pub(crate) enum PrecisionHelp {
    /// An `i64` part, which `f64(...)` converts: the part.
    Convert(Span),
    /// An `f64?` part, which `??` can give a default: the part and the spec, with its `:`, as
    /// written; the part has parentheses when `??` needs them.
    Default { part: String, spec: String },
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

/// A `receiver?.member` whose receiver is an optional of an optional, so `?.` leaves `left`.
#[derive(Clone)]
pub(crate) struct Peel {
    /// The receiver as written.
    pub(crate) receiver: String,
    pub(crate) left: String,
    pub(crate) member: String,
    /// How many `?? None` the help puts around the receiver.
    pub(crate) layers: usize,
    /// Whether the diagnostic has the peel help. The peeled member may fail too.
    pub(crate) help: bool,
}

/// What the help of a `MayBeNone` tells the user to unwrap with `if let`.
#[derive(Clone)]
pub(crate) enum UnwrapHelp {
    /// The optional itself.
    It,
    /// The object, as written, of the `?.` that makes an element place or a pushed array optional.
    Object(String),
    /// Nothing: no `if let` of the optional gives a place that can be written.
    Nothing,
    /// The optional struct value `base`, as written: an unwrap gives a copy, so the help says to
    /// replace the value.
    Replace(String),
    /// A `.` on an optional of an optional: `?.` takes one layer only, so the help peels the
    /// other layers first. `receiver` and `member` are as written; `layers` is the count of
    /// `?? None`.
    Peel {
        receiver: String,
        member: String,
        layers: usize,
    },
}

/// The help of an E0062 that says what to unwrap. With `fix`, a `?.` fix comes beside it, so the
/// help also says why to unwrap.
fn unwrap_text(unwrap: UnwrapHelp, fix: bool) -> Option<String> {
    let lead = if fix {
        "to use the value only when it is present, unwrap"
    } else {
        "unwrap"
    };
    match unwrap {
        UnwrapHelp::It => Some(format!(
            "{lead} it with `if let`, `let ... else` or `?? return`"
        )),
        UnwrapHelp::Object(object) => Some(format!(
            "{lead} `{object}` with `if let`, `let ... else` or `?? return`"
        )),
        UnwrapHelp::Peel {
            receiver,
            member,
            layers,
        } => Some(peel_text("extra", &receiver, &member, layers)),
        UnwrapHelp::Replace(base) => Some(replace_text(&base)),
        UnwrapHelp::Nothing => None,
    }
}

#[cfg(test)]
thread_local! {
    /// How many words `string_index_names` has put into its set or looked up, on this thread.
    pub(crate) static NAME_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The names `bs`, `cs` and `i` that the help of a single index of a string writes. A name that
/// `base` holds as a whole word gets the first number from 2 that `base` does not hold. So the
/// help does not name `base` twice.
pub(crate) fn string_index_names(base: &str) -> [String; 3] {
    let words: FxHashSet<&str> = base
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .inspect(|_| {
            #[cfg(test)]
            NAME_STEPS.set(NAME_STEPS.get() + 1);
        })
        .collect();
    ["bs", "cs", "i"].map(|name| {
        let mut fresh = name.to_owned();
        let mut number = 1;
        while {
            #[cfg(test)]
            NAME_STEPS.set(NAME_STEPS.get() + 1);
            words.contains(fresh.as_str())
        } {
            number += 1;
            fresh = format!("{name}{number}");
        }
        fresh
    })
}

/// The help that replaces the optional struct value `base`, as written.
fn replace_text(base: &str) -> String {
    format!("replace the value: `{base} = Some(...)`")
}

/// The help "peel the `which` layer first": `receiver` with `layers` of `?? None` around it, then
/// `?.` and `member`. It says "layers" for more than one.
fn peel_text(which: &str, receiver: &str, member: &str, layers: usize) -> String {
    let plural = if layers > 1 { "s" } else { "" };
    let mut text = format!("peel the {which} layer{plural} first: `");
    text.reserve(receiver.len() + member.len() + layers * 10 + 4);
    text.extend(std::iter::repeat_n('(', layers));
    text.push_str(receiver);
    for _ in 0..layers {
        text.push_str(" ?? None)");
    }
    text.push_str("?.");
    text.push_str(member);
    text.push('`');
    text
}

/// Moving a token up to the code before it: the `gap` between them becomes a space.
#[derive(Clone, Copy)]
pub(crate) struct Join {
    pub(crate) gap: Span,
    /// Whether the move is always right, not only likely.
    pub(crate) safe: bool,
}

/// What an `else` at the start of a line should follow (E0006).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ElseOf {
    /// The `}` that closes an `if`.
    If,
    /// The value of a `let`, in `let pattern = value else { ... }`.
    Let,
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
    /// `.`. It is `None` when no pattern follows, so removing them leaves an arm with no pattern.
    Case { remove: Option<Vec<Span>> },
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
    /// Labels on the sub-patterns; `labels` are each `label: ` the fix deletes. It is `None` when
    /// a comment lies in one of them, which the fix would delete.
    Labels {
        form: String,
        labels: Option<Vec<Span>>,
    },
    /// Rust's braces, `Rect { w, h }`; `fix` is what the fix replaces, from the end of the name
    /// through the `}`, and the `(w, h)` that replaces it. It is `None` when a comment lies
    /// between the `{`, the sub-patterns and the `}`, which the fix would delete.
    Braces {
        form: String,
        fix: Option<(Span, String)>,
    },
    /// Type arguments, `Option<i64>.Some(v)`; the fix deletes them, unless `fix` is false because
    /// a comment lies in them. The type of the matched value gives them.
    TypeArgs { fix: bool },
}

/// A range written another language's way. `start` and `end` are its bounds as written; each is
/// `None` when it is missing or did not parse, and the message then calls it `start` or `end`.
pub(crate) enum ForeignRange {
    /// `start..<end`; `remove` is the `<`, unless removing it leaves a range the parser rejects.
    /// `start` is `None` when the range has none, and the message calls it `start`.
    Exclusive {
        start: Option<String>,
        end: Option<String>,
        remove: Option<Span>,
    },
    /// `start...end`.
    Ellipsis {
        start: Option<String>,
        end: Option<String>,
    },
    /// `base[start:end]`, as Python and Go write a slice. `at` is the `:`, or the first negative
    /// bound when a bound is negative. A bound that is left out is `None`. `whole` is the `[:]`
    /// of `base[:]`, which the fix removes. `base_place` is true when `base` is a place that starts
    /// at a name or `self`, so that to count from `base.len()` reads the same place.
    Colon {
        base: String,
        base_place: bool,
        start: Option<ColonBound>,
        end: Option<ColonBound>,
        whole: Option<Span>,
    },
}

/// A bound of `base[start:end]`. Python counts a negative one from the end of `base`; Sisu has no
/// negative index.
pub(crate) enum ColonBound {
    /// The text of a bound that is not negative.
    Plain(String),
    /// A bound that starts with a minus, as `-1`, `-a * b` or `--1`.
    Negative,
}

/// How a range is wrong where the checker finds it.
pub(crate) enum RangeForm {
    /// A range with both bounds, where only a `for` source, a pattern, an index or a string's
    /// `get` argument can be one.
    Anywhere,
    /// A range with a missing bound, where only an index or a string's `get` can have one.
    MissingBound {
        missing: MissingBound,
        place: RangePlace,
    },
    /// `start..=` with no end in an index or a string's `get`, which is no range there: `quote` is
    /// its source text and the source text of `start`, each on one line, or `None` when one of
    /// them has no such text.
    NoEnd { quote: Option<(String, String)> },
    /// `base[..]` or `base[..=]`: the whole of `base`, which is `base`. `base` is `None` when it
    /// has no one-line text.
    NoBounds {
        base: Option<String>,
        inclusive: bool,
    },
    /// `base.get(..)` or `base.get(..=)`, a string's `get` of its whole value, written as `call`.
    /// `base` is `None` when it has no one-line text.
    NoBoundsGet {
        base: Option<String>,
        inclusive: bool,
        call: GetCall,
    },
}

/// How the `get` of a string is written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GetCall {
    /// `base.get(..)`.
    Plain,
    /// `base?.get(..)` on an optional: it is `base`.
    Safe,
    /// `base?.get(..)` where the `?.` failed (E0049 or E0062), so the call has no value to name.
    SafeFailed,
    /// `base.get(..)` on an optional `base`, checked on its payload. E0062 is the error, so the
    /// call has no value to name.
    Payload,
}

/// What an index into a string does with its element.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexUsage {
    /// The element is read, or only part of a longer target.
    Read,
    /// The element is the whole target of an assignment.
    Write,
}

/// Which bound of a range is missing.
pub(crate) enum MissingBound {
    Start,
    End,
    /// `..`: both.
    Both,
    /// `..=`: both.
    BothInclusive,
}

/// Where a range with a missing bound stands, which gives the help.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangePlace {
    ForSource,
    Elsewhere,
}

impl RangeForm {
    fn diagnostic(self, code: Code, at: Span) -> Diagnostic {
        const MISSING: &str =
            "a range with a missing bound can only be an index or a string's `get` argument";
        match self {
            Self::Anywhere => Diagnostic::coded(
                code,
                at,
                "a range can only be a `for` source, a pattern, an index or a string's `get` argument",
            ),
            Self::MissingBound { missing, place } => {
                let label = match missing {
                    MissingBound::Start => "this range has no start",
                    MissingBound::End => "this range has no end",
                    MissingBound::Both | MissingBound::BothInclusive => "this range has no bounds",
                };
                let help = match (missing, place) {
                    (MissingBound::Both, _) => {
                        "`..` alone is the whole value; write the value itself"
                    }
                    (_, RangePlace::ForSource) => {
                        "write both bounds, such as `0..n`, or use `loop` with a counter"
                    }
                    (_, RangePlace::Elsewhere) => {
                        "write the range where it is used: in an index or in a string's `get`"
                    }
                };
                Diagnostic::coded(code, at, MISSING)
                    .with_label(label)
                    .with_help(help)
            }
            Self::NoEnd { quote: None } => {
                Diagnostic::coded(code, at, "this `..=` range has no end")
                    .with_label("this range has no end")
                    .with_help("a `..=` range must have an end")
            }
            Self::NoEnd {
                quote: Some((range, start)),
            } => Diagnostic::coded(code, at, format!("`{range}` has no end"))
                .with_label("this range has no end")
                .with_help(format!("write `{start}..`")),
            Self::NoBounds { base, inclusive } => {
                let dots = if inclusive { "..=" } else { ".." };
                let (message, help) = match base {
                    None => (
                        format!("`[{dots}]` has no bounds"),
                        format!("`[{dots}]` gives the whole value, so it has no effect"),
                    ),
                    Some(base) if inclusive => (
                        format!("`{base}[{dots}]` has no bounds"),
                        format!("write `{base}`"),
                    ),
                    Some(base) => (
                        format!("`{base}[{dots}]` has no bounds"),
                        format!("`{base}[..]` is `{base}`; write `{base}`"),
                    ),
                };
                Diagnostic::coded(code, at, message)
                    .with_label("this range has no bounds")
                    .with_help(help)
            }
            Self::NoBoundsGet {
                base,
                inclusive,
                call: how,
            } => {
                let dots = if inclusive { "..=" } else { ".." };
                let Some(base) = base else {
                    return Diagnostic::coded(code, at, format!("`get({dots})` has no bounds"))
                        .with_label("this range has no bounds");
                };
                let (call, value) = match how {
                    GetCall::Plain => (format!("{base}.get"), Some(format!("Some({base})"))),
                    GetCall::Safe => (format!("{base}?.get"), Some(base)),
                    // The call is not `base`, so no help names a value for it.
                    GetCall::SafeFailed => (format!("{base}?.get"), None),
                    GetCall::Payload => (format!("{base}.get"), None),
                };
                let d = Diagnostic::coded(code, at, format!("`{call}({dots})` has no bounds"))
                    .with_label("this range has no bounds");
                match value {
                    Some(value) if inclusive => d.with_help(format!("write `{value}`")),
                    Some(value) => d.with_help(format!("`{call}(..)` is `{value}`")),
                    None => d,
                }
            }
        }
    }
}

/// A range index in the target of an assignment, as written: the source text of its parts, which
/// the help repeats so that a reader can copy it.
pub(crate) struct RangeTarget {
    /// The base of the range index.
    pub(crate) base: String,
    /// The start, `None` when missing.
    pub(crate) start: Option<String>,
    /// The end that `..end` writes: for `..=b`, the next value, `b + 1`. `None` when missing.
    pub(crate) end: Option<String>,
    pub(crate) write: RangeWrite,
}

/// What the assignment does with the range index.
pub(crate) enum RangeWrite {
    /// `xs[a..b] op v` on an array.
    Array,
    /// `xs[a..b][i]tail op v` on an array, or `xs[a..b][i]tail.push(v)`: `index` is the index
    /// into `xs` that `i` stands for, `a + i`, or `i` when there is no `a`, and `tail` the indexes
    /// after it, if any. `store` is what follows: ` = v`, ` += v`, or `.push(v)`.
    Element {
        index: String,
        tail: String,
        store: String,
    },
    /// `s[a..b] = t` on a string: `value` is `t`, in parentheses if it is more than one term.
    Replace { value: String },
    /// `s[a..b] += t` on a string: `value` is `t`, and `grouped` is `t` in parentheses if it is
    /// more than one term.
    Append { value: String, grouped: String },
    /// Any other compound assignment to a range of a string.
    Other,
    /// Any of these on a base that is no place, such as a call: no write to it is allowed.
    Temporary,
}

/// The range index in the receiver of a `push` that is no place.
pub(crate) enum RangePush {
    /// The receiver is the range index: the source text of it.
    Slice(String),
    /// The receiver is an element of the range index.
    Element(RangeTarget),
}

impl RangeTarget {
    fn diagnostic(self, code: Code, at: Span) -> Diagnostic {
        let d = Diagnostic::coded(code, at, "cannot assign to this expression")
            .with_label("a range index gives a new value");
        match self.help() {
            Some(help) => d.with_help(help),
            None => d,
        }
    }

    /// The help, or `None` where no help fits yet.
    fn help(self) -> Option<String> {
        let Self {
            base,
            start,
            end,
            write,
        } = self;
        Some(match write {
            RangeWrite::Array => format!("assign to each element, `{base}[i] = ...`, in a loop"),
            RangeWrite::Element { index, tail, store } => {
                format!("write `{base}[{index}]{tail}{store}`; a range index counts from its start")
            }
            RangeWrite::Replace { value } => {
                let before = start.map(|start| format!("{base}[..{start}] + "));
                let after = end.map(|end| format!(" + {base}[{end}..]"));
                format!(
                    "build the new string, such as `{base} = {}{value}{}`",
                    before.unwrap_or_default(),
                    after.unwrap_or_default()
                )
            }
            RangeWrite::Append { value, grouped } => match end {
                Some(end) => format!(
                    "build the new string, such as `{base} = {base}[..{end}] + {grouped} + {base}[{end}..]`"
                ),
                None => format!("write `{base} += {value}`"),
            },
            RangeWrite::Other => return None,
            RangeWrite::Temporary => format!("store `{base}` in a `let mut` variable first"),
        })
    }
}

/// The name of `NotVariantHere` with its first letter in lower case, and what that spells.
pub(crate) enum LowerName {
    /// No keyword: a binding. `true` and `false` are here too, as they are patterns.
    Binding(String),
    /// A keyword other than `true` and `false`.
    Keyword(String),
    /// A reserved word.
    Reserved(String),
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

/// A parameter of a requirement whose `mut` the method does not share.
#[derive(Clone)]
pub(crate) struct MutDiffers {
    /// Where the requirement's parameter is named.
    pub(crate) at: Span,
    /// Its name.
    pub(crate) name: String,
    /// Whether the requirement's parameter is `mut`.
    pub(crate) mutable: bool,
}

/// The binding that W0002 names.
#[derive(Clone)]
pub(crate) enum NeverReassignedForm {
    /// A `let mut`. The fix removes `remove`: the `mut` and the white space after it, up to a
    /// comment.
    Let { remove: Span },
    /// A `mut` parameter, of a class type when `class`. Removing its `mut` changes every caller,
    /// so there is no fix.
    Param { class: bool },
}

/// What owns a method.
#[derive(Clone)]
pub(crate) enum Owner {
    Class,
    Enum(String),
    Interface,
    Struct,
}

/// What is wrong with a conformance.
#[derive(Clone)]
pub(crate) enum ConformanceForm {
    /// The signatures of the requirements that no method meets. `generated` says why the compiler
    /// writes none for a language interface.
    Missing {
        signatures: Vec<String>,
        generated: Option<Ungenerated>,
    },
    /// The method `method`, written as `found` at the diagnostic's span, which the requirement
    /// `want` does not accept.
    Wrong {
        method: String,
        found: String,
        want: String,
        mut_differs: Option<MutDiffers>,
    },
    /// The method `method` of an `extend` block, which is no requirement; `target` is the type
    /// that its own block would extend.
    NotRequirement { method: String, target: String },
    /// The choice of the associated type `name`, which none of the interfaces `of` declares.
    NotAssociated { name: String, of: Vec<String> },
}

/// Why the compiler writes no conformance of a language interface for a type.
#[derive(Clone)]
pub(crate) enum Ungenerated {
    /// It writes none for the type at all: a class has no `Eq`, and nothing has an `Ord` but
    /// the built-in numbers, `char` and `string`.
    Never,
    /// The rule for the type needs `leaf` to have the interface, and it has none.
    Since(String),
}

/// Why an interface cannot be `any` (`CannotBeAny`).
#[derive(Clone)]
pub(crate) enum NotAny {
    /// The requirement `method` takes the type that has it, `Self`, in a place that is not its
    /// receiver.
    TakesSelf { method: String },
    /// The requirement `method` returns `ret`, which names `Self`.
    Returns { method: String, ret: String },
    /// The requirement `method` has type parameters of its own.
    Generic { method: String },
    /// The interface `owner` has the associated type `assoc`.
    Assoc { owner: String, assoc: String },
    /// The interface inherits `inherited`, written with its arguments, one of which names `Self`.
    InheritsSelf { inherited: String },
}

/// How `any` is written wrongly (`AnyForm`).
#[derive(Clone)]
pub(crate) enum AnyFormKind {
    /// `any I?`: `bare`, which is `written`, is `any I`, and `layers` `?` follow it.
    Optional {
        written: String,
        bare: Span,
        layers: usize,
    },
    /// `any A & B`, with the interfaces as written.
    Several { first: String, second: String },
}

/// Why an `extend` block cannot extend.
#[derive(Clone)]
pub(crate) enum CannotExtendForm {
    /// The target is a built-in type.
    BuiltIn,
    /// The target is a type parameter.
    TypeParam,
    /// A type parameter of the block that `target`, as written, does not name. `remove` is the
    /// text from `extend` to the target, which holds the list of type parameters, when the fix
    /// may take the list out and leave one space.
    Unused {
        target: String,
        remove: Option<Span>,
    },
    /// The target or an interface argument, as written, names an associated type.
    Associated,
    /// The target is an `any` type, an interface value.
    Any,
}

/// Why a type must conform to an interface.
#[derive(Clone)]
pub(crate) enum NotConformForm {
    /// The type conforms to `via`, at the diagnostic's span, which requires `interface` at
    /// `requires`, nowhere a user can see for the prelude's. `extend` is the block that would add
    /// the conformance, when the type has none.
    Inherited {
        via: String,
        requires: Option<Span>,
        extend: Option<String>,
        /// A conformance the type has already, whose bound fails: the bound as written there,
        /// and where it is.
        needs: Option<(String, Span)>,
        /// The help for that bound: the bound to add to a type parameter, or the class or enum
        /// to make conform.
        add: Option<AddBound>,
        make: Option<Make>,
    },
    /// The type is converted to `any I`, which asks for the interface. `origin` is where that
    /// `any I` comes from: the declared type, a parameter, a return, or the member of a join.
    Any { origin: Option<MismatchOrigin> },
    /// A bound of a declaration, written at `bound` (nowhere a user can see for the prelude's),
    /// needs it. `implied` holds the text of an associated type and of its base, when `bound` is
    /// that associated type: no one writes the bound there. `add` is the bound that a type
    /// parameter would need. A bound of the prelude has `label` in its place, and so has a bound
    /// of the program that nothing else names (see `BoundLabel`). `make` is the class or enum
    /// with no conformance, which the help shows how to give one.
    Bound {
        bound: Option<Span>,
        implied: Option<Implied>,
        label: Option<BoundLabel>,
        make: Option<Make>,
        /// Whether the interface is `AnyObject`, which only classes have.
        class_only: bool,
        add: Option<AddBound>,
        /// A conformance the type has already, whose bound fails: the bound as written there,
        /// and where it is.
        needs: Option<(String, Span)>,
    },
}

/// An associated type, as written, and its base: the text of an implied bound.
#[derive(Clone)]
pub(crate) struct Implied {
    pub(crate) written: String,
    pub(crate) base: String,
}

/// How the label names a bound that fails: on the call, for a bound of the prelude or of a
/// block, and on the type that holds the part, for a bound of the program.
#[derive(Clone)]
pub(crate) enum BoundLabel {
    /// The type itself fails the bound.
    Call { need: Option<BlockNeed> },
    /// A part of the type fails the bound: `part` does not conform to the interface.
    Since {
        part: String,
        need: Option<BlockNeed>,
    },
    /// A bound that the program writes, with no call of a method: the type holds `part`, which
    /// does not conform to the interface.
    Holds { part: String },
}

impl BoundLabel {
    /// The words of the label for a type `ty` that fails `interface`.
    fn text(&self, ty: &str, interface: &str) -> String {
        match self {
            BoundLabel::Call { need: None } => "required by the bound of this call".to_string(),
            BoundLabel::Call { need: Some(need) } => {
                format!("`{}` needs `{}: {interface}`", need.method, need.stands)
            }
            BoundLabel::Since { part, need: None } => format!(
                "required by the bound of this call, since `{part}` does not conform to `{interface}`"
            ),
            BoundLabel::Since {
                part,
                need: Some(need),
            } => format!(
                "`{}` needs `{}: {interface}`, and `{part}` does not conform to `{interface}`",
                need.method, need.stands
            ),
            BoundLabel::Holds { part } => {
                format!("`{ty}` holds `{part}`, which does not conform to `{interface}`")
            }
        }
    }
}

/// A class or an enum that the help says to make conform to an interface, written with its
/// arguments. It is the type itself, or a part of the type that fails a bound of a conformance
/// that the program writes.
#[derive(Clone)]
pub(crate) struct Make {
    pub(crate) ty: String,
    pub(crate) interface: String,
}

/// The help that makes a type meet an interface: the bound to add, or the class or enum to give a
/// conformance.
fn conform_help(add: Option<&AddBound>, make: Option<&Make>) -> Option<String> {
    match (add, make) {
        (Some(add), _) => Some(add.help()),
        (None, Some(Make { ty, interface })) => {
            Some(format!("make `{ty}` conform to `{interface}`"))
        }
        (None, None) => None,
    }
}

/// The bound of a block that fails for the receiver of a method that a call writes as a function,
/// as `NotConform` names it on a call of the method.
#[derive(Clone)]
pub(crate) struct BlockBound {
    pub(crate) interface: String,
    pub(crate) label: BoundLabel,
    pub(crate) make: Option<Make>,
    pub(crate) add: Option<AddBound>,
    /// Whether the method can take the other arguments of the call. The help says to fix them
    /// when it cannot. In the `self` form, only the count, a label or a type argument decide it:
    /// the checker types the first argument before it knows the method.
    pub(crate) takes: bool,
}

/// The method of a prelude block that has the bound, and the type that stands for the block's
/// type parameter at the call: the program wrote that type, not the name in the prelude. The
/// label names them when the call is of that method.
#[derive(Clone)]
pub(crate) struct BlockNeed {
    pub method: String,
    pub stands: String,
}

/// What the report of a missing operator (`NoOperator`) says besides the missing operator.
#[derive(Clone)]
pub(crate) enum NoOperatorForm {
    /// `==` or `!=` on a class, which compares by identity. The help shows `is` too.
    Identity { make: String },
    /// `==` or `!=` on an `any` type, which has no `Eq`. The help shows `is` when the interface
    /// is class-only (`is`).
    Any { is: bool },
    /// An operator of the type, with the help to make `make` conform: the type itself, or the
    /// part inside a type whose conformance has a bound that fails.
    Operator { make: Option<Make> },
}

/// Where a missing bound goes.
#[derive(Clone)]
pub(crate) enum BoundSite {
    /// A type parameter, by name.
    Param(String),
    /// An associated type, by name, in the `interface` that declares it.
    Assoc { interface: String, name: String },
}

/// A bound that a type parameter or an associated type lacks: `site`, then every bound it would
/// have, in order.
#[derive(Clone)]
pub(crate) struct AddBound {
    pub(crate) site: BoundSite,
    pub(crate) bounds: Vec<String>,
}

impl AddBound {
    /// The help that tells what to write.
    fn help(&self) -> String {
        let bounds = self.bounds.join(" & ");
        match &self.site {
            BoundSite::Param(param) => format!("add a bound: `{param}: {bounds}`"),
            BoundSite::Assoc { interface, name } => {
                format!("add a bound in `{interface}`: `type {name}: {bounds}`")
            }
        }
    }
}

/// An `is` with the extra layers of its operands taken off, as written, and how many layers the
/// help says to take off.
#[derive(Clone)]
pub(crate) struct PeelIs {
    pub(crate) text: String,
    pub(crate) layers: PeelLayers,
}

/// The layers that the help of an `is` peels.
#[derive(Clone, Copy)]
pub(crate) enum PeelLayers {
    /// One side has one extra layer; the other has none.
    One,
    /// Both sides have one extra layer.
    OfEachSide,
    /// A side has more than one extra layer.
    Extra,
}

/// The `let` that picks one answer of an ambiguous call: the call as written, the name of the
/// binding, and the type that exactly one answer returns.
#[derive(Clone)]
pub(crate) struct Annotate {
    pub(crate) call: String,
    pub(crate) binding: String,
    pub(crate) ty: String,
}

/// What an interface holds that it should not.
#[derive(Clone, Copy)]
pub(crate) enum InterfaceBodyForm {
    /// A `{` after a requirement.
    Body,
    /// A field.
    Field,
}

/// What a built-in name is, and so why a program cannot take it.
#[derive(Clone, Copy)]
pub(crate) enum BuiltIn {
    /// The name of a built-in type.
    Type,
    /// The name of an interface the language declares: `Display`, `Eq`, `Ord` or `AnyObject`.
    Interface,
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
    /// An enum or a struct, and the links that lead from it back to it: each holder (`D.Q` or
    /// `K.d`) and the type it holds. `first_struct` is the first struct on the chain, which the
    /// help names.
    Chain {
        links: Vec<(String, String)>,
        first_struct: Option<String>,
    },
}

/// What the report of a missing `mut` offers for the argument.
pub(crate) enum MissingFix {
    /// The place fits the parameter: the safe fix that adds `mut `.
    Add,
    /// A place that `mut` would still make an error (E0170 or E0171), or that is not known: no
    /// fix and no help.
    NoFix,
    /// A `let` local `name`, passed as `arg`.
    Let { name: String, arg: String },
    /// A field `name` that is not `mut`.
    Field { name: String },
    /// Anything else: only a local copy can change.
    Copy,
}

/// How a `mut` argument at a parameter that is not `mut` names its callee.
pub(crate) enum NotMutForm {
    /// A callee that the program writes, by the parameter `name` of `callee`.
    Written {
        name: String,
        callee: String,
        declared: Option<Span>,
    },
    /// A built-in, by the `position` of the argument, from 1, and the `callee` as written.
    Unnamed { position: usize, callee: String },
    /// The constructor of a class, written `shown`.
    Class { shown: String },
    /// A variant, written `shown`.
    Variant { shown: String },
}

/// Why an argument cannot be passed `mut`.
pub(crate) enum NotPlace {
    /// A binding or a field that cannot change.
    Immutable(Immutable),
    /// `self`, where `class` says that its type is a class.
    SelfValue { class: bool },
    /// A value that is no place: a call, a literal.
    Temporary,
    /// A range index of the kind of value `SliceOf` says.
    Range(SliceOf),
}

/// What a range index gives: a new `string` or a new array.
#[derive(Clone, Copy)]
pub(crate) enum SliceOf {
    String,
    Array,
}

/// The `?.` form of `MutArgumentType`: the place reads as an optional and is written as what
/// the field stores.
pub(crate) struct ChainForm {
    /// The place as written, when it is on one line.
    pub(crate) shown: Option<String>,
    pub(crate) label: ChainLabel,
    /// A sentence for the help, when unwrapping the object gives a place that fits.
    pub(crate) help: Option<String>,
}

/// Which type of a `?.` place the parameter's type misses.
pub(crate) enum ChainLabel {
    /// The parameter has the type that the place is written as.
    WriteType,
    /// The parameter has the type that the place reads as, and the field `field` cannot hold it.
    WritesNone { field: String },
    /// The place reads as what it is written as: no place exists when `head` is `None`.
    NoPlace { head: Option<String> },
    /// Any other type.
    Other,
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
    /// The plain `self` of a struct method.
    StructSelf,
}

/// What a write changes of its root.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Written {
    /// The root itself: `t = v`.
    Whole,
    /// An element of the root: `t[i] = v`.
    Element,
    /// A part of the root, through a struct field: `t.n = v` or `t.i.xs[0] = v`.
    Part,
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
                written,
                cause,
            } => {
                let what = match written {
                    Written::Whole => "",
                    Written::Element => "an element of ",
                    Written::Part => "a part of ",
                };
                let d = Diagnostic::coded(code, at, format!("cannot assign to {what}`{name}`"));
                immutable_cause(d, &name, written != Written::Whole, &cause)
            }
            DiagnosticKind::PushToImmutable { name, at, cause } => {
                let d = Diagnostic::coded(code, at, format!("cannot change `{name}`"))
                    .with_label(format!("`push` changes `{name}`"));
                immutable_cause(d, &name, true, &cause)
            }
            DiagnosticKind::UnreachableCode { at } => {
                Diagnostic::coded(code, at, "unreachable code")
            }
            DiagnosticKind::NeverReassigned {
                name,
                at,
                form: NeverReassignedForm::Let { remove },
            } => Diagnostic::coded(code, at, format!("`{name}` is never reassigned")).with_fix(Fix {
                message: "declare it with `let`".to_string(),
                safe: true,
                edits: vec![Edit {
                    span: remove,
                    replacement: String::new(),
                }],
            }),
            DiagnosticKind::NeverReassigned {
                name,
                at,
                form: NeverReassignedForm::Param { class },
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` is never reassigned, so its `mut` changes nothing for the caller"),
                )
                .with_label("never reassigned");
                if class {
                    d.with_help("the fields of a class object change without `mut`")
                } else {
                    d
                }
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
                    "`{` must be on the same line as `fn`, `class`, `struct`, `enum`, `interface`, `extend`, `if`, `else`, `match`, `while`, `for` or `loop`",
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
            DiagnosticKind::ElseOnNewLine { at, of, join } => {
                let (message, fix, unsafe_fix) = match of {
                    ElseOf::If => (
                        "`else` must be on the same line as the closing `}`",
                        "move `else` after the `}`",
                        "if this `else` belongs to the `if` above, move it after the `}`",
                    ),
                    ElseOf::Let => (
                        "`else` must be on the same line as the value of the `let`",
                        "move `else` after the value",
                        "if this `else` belongs to the `let` above, move it after the value",
                    ),
                };
                let d = Diagnostic::coded(code, at, message);
                match join {
                    Some(join) => d.with_fix(join_fix(fix, unsafe_fix, join)),
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
                let d = Diagnostic::coded(code, at, "base prefixes are lower case");
                match safe {
                    Some(safe) => d.with_fix(Fix {
                        message: "write the prefix in lower case".to_string(),
                        safe,
                        edits: vec![Edit {
                            span: Span::new(at.end() - 1, at.end()),
                            replacement: prefix.to_ascii_lowercase().to_string(),
                        }],
                    }),
                    None => d,
                }
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
            DiagnosticKind::LogicalOperatorWritten { c, at } => {
                let d = Diagnostic::coded(code, at, format!("unexpected character {}", shown(c)));
                d.with_fix(match c {
                    '&' => unsafe_fix("if you meant logical and, use `&&`", at, "&&"),
                    '|' => unsafe_fix("if you meant logical or, use `||`", at, "||"),
                    _ => unreachable!("a logical operator should be `&` or `|`"),
                })
            }
            DiagnosticKind::Expected { what, found, at } => {
                Diagnostic::coded(code, at, format!("expected {what}, found {found}"))
            }
            DiagnosticKind::JumpTakesBlock {
                keyword,
                at,
                grouped,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{keyword}` takes the `{{ ... }}` after it as its value"),
                );
                match grouped {
                    Some(grouped) => d.with_help(format!(
                        "to keep the block as the body, put the jump in `( )`: `({grouped})`"
                    )),
                    None => d,
                }
            }
            DiagnosticKind::LetTakesNoType {
                at,
                with_else,
                join,
            } => {
                let what = if with_else {
                    "`let ... else`"
                } else {
                    "`let` pattern"
                };
                // The parser stops at the type, so it has not read the rest of the statement: the
                // fixed source can have an error of its own, and the fix is not safe. An `else` on
                // a later line moves after the value, as E0006 would ask next.
                let mut fix = unsafe_fix("remove the type", at, "");
                if let Some(gap) = join {
                    fix.message = "remove the type, and move `else` after the value".to_string();
                    fix.edits.push(Edit {
                        span: gap,
                        replacement: " ".to_string(),
                    });
                }
                Diagnostic::coded(code, at, format!("a {what} takes no type")).with_fix(fix)
            }
            DiagnosticKind::Var { at, field } => {
                let d = Diagnostic::coded(code, at, "there is no `var` in Sisu");
                if field {
                    d.with_help("a field that can change is marked `mut`")
                } else {
                    d.with_help("Sisu declares a variable with `let mut`")
                }
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
                remove,
                annotation,
                safe,
                owner,
            } => {
                // The fix removes the `mut`, and the type of a typed receiver with it: `self`
                // takes no type, so the source would still not parse.
                let drop_mut = |d: Diagnostic, safe: bool, message: &str| {
                    let message = if annotation.is_some() {
                        "remove the `mut` and the type"
                    } else {
                        message
                    };
                    d.with_fix(Fix {
                        message: message.to_string(),
                        safe,
                        edits: std::iter::once(remove)
                            .chain(annotation)
                            .map(|span| Edit {
                                span,
                                replacement: String::new(),
                            })
                            .collect(),
                    })
                };
                // The fix of a class or an interface method: safe when only `,` or `)` follows
                // `self`.
                let message = if safe {
                    "remove the `mut`"
                } else {
                    "try removing the `mut`; the receiver may need another change"
                };
                match owner {
                    Owner::Class => drop_mut(
                        Diagnostic::coded(
                            code,
                            keyword,
                            "a class method takes `self`, not `mut self`",
                        )
                        .with_help("an object is shared: `self` can already write every `mut` field"),
                        safe,
                        message,
                    ),
                    Owner::Interface => drop_mut(
                        Diagnostic::coded(
                            code,
                            keyword,
                            "an interface method cannot take `mut self`",
                        )
                        .with_help(
                            "a class method changes its object's `mut` fields through plain `self`",
                        ),
                        safe,
                        message,
                    ),
                    // The body may still assign `self`, so the fix is never safe.
                    Owner::Enum(name) => drop_mut(
                        Diagnostic::coded(
                            code,
                            keyword,
                            format!("`{name}` is an enum, and enum values never change"),
                        )
                        .with_help(format!("return a new `{name}`")),
                        false,
                        "remove the `mut`",
                    ),
                    // The body may still write through `self`, so no fix is exact.
                    Owner::Struct => Diagnostic::coded(
                        code,
                        keyword,
                        "a struct method cannot take `mut self` yet",
                    )
                    .with_help("take `self` and return the changed copy"),
                }
            }
            DiagnosticKind::InterfaceBody { at, form } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    "an interface holds only method signatures and associated types",
                );
                match form {
                    InterfaceBodyForm::Body => d.with_help("write the body in each conforming type"),
                    InterfaceBodyForm::Field => d,
                }
            }
            DiagnosticKind::InterfaceAsType {
                name,
                written,
                at,
                any,
                layers,
                alias,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{name}` is an interface, not a type"),
                );
                // `Shape?` takes `(any Shape)?`, which has parentheses and keeps each `?`.
                let value = if layers == 0 {
                    format!("any {}", written.as_deref().unwrap_or_default())
                } else {
                    format!(
                        "(any {}){}",
                        written.as_deref().unwrap_or_default(),
                        "?".repeat(layers)
                    )
                };
                match written {
                    Some(_) if alias && any => d.with_help(format!(
                        "use `{value}` for a value of any conforming type"
                    )),
                    Some(_) if alias => d,
                    Some(written) if any => d.with_help(format!(
                        "use a type parameter: `T: {written}`, or `{value}` for a value of any conforming type"
                    )),
                    Some(written) => d.with_help(format!("use a type parameter: `T: {written}`")),
                    None => d,
                }
            }
            DiagnosticKind::ConformanceBody {
                ty,
                interface,
                at,
                form,
            } => match form {
                ConformanceForm::Missing {
                    signatures,
                    generated,
                } => {
                    let d =
                        Diagnostic::coded(code, at, format!("`{ty}` does not meet `{interface}`"));
                    let d = match generated {
                        None => d,
                        Some(Ungenerated::Never) => {
                            d.with_label(format!("`{ty}` gets no generated `{interface}`"))
                        }
                        Some(Ungenerated::Since(leaf)) => d.with_label(format!(
                            "`{ty}` gets no generated `{interface}`, since `{leaf}` has none"
                        )),
                    };
                    d.with_help(signatures.join("\n"))
                }
                ConformanceForm::Wrong {
                    method,
                    found,
                    want,
                    mut_differs,
                } => {
                    let d = Diagnostic::coded(
                        code,
                        at,
                        format!("`{method}` does not match `{interface}`'s requirement"),
                    )
                    .with_label(format!("found `{found}`"))
                    .with_help(format!("`{interface}` requires `{want}`"));
                    match mut_differs {
                        Some(MutDiffers { at, name, mutable }) => {
                            let not = if mutable { "" } else { "not " };
                            d.with_secondary(
                                at,
                                format!("the requirement's parameter `{name}` is {not}`mut`"),
                            )
                        }
                        None => d,
                    }
                }
                ConformanceForm::NotRequirement { method, target } => Diagnostic::coded(
                    code,
                    at,
                    format!("`{method}` is not a requirement of `{interface}`"),
                )
                .with_help(format!("move it to `extend {target} {{ ... }}`")),
                ConformanceForm::NotAssociated { name, of } => Diagnostic::coded(
                    code,
                    at,
                    format!(
                        "`{name}` is not an associated type of {}",
                        listed(&of, "or")
                    ),
                ),
            },
            DiagnosticKind::ConformsTwice {
                ty,
                interface,
                at,
                first,
                generic,
            } => {
                let d = Diagnostic::coded(code, at, format!("`{ty}` conforms to `{interface}` twice"))
                    .with_secondary(first, "first conformance here");
                if generic {
                    d.with_help(
                        "a type conforms to an interface once for each list of the interface's type arguments",
                    )
                } else {
                    d
                }
            }
            DiagnosticKind::NotConform {
                ty,
                interface,
                at,
                form,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{ty}` does not conform to `{interface}`"),
                );
                match form {
                    NotConformForm::Inherited {
                        via,
                        requires,
                        extend,
                        needs,
                        add,
                        make,
                    } => {
                        let note = format!("`{via}` requires `{interface}`");
                        let d = match requires {
                            Some(requires) => d.with_secondary(requires, note),
                            None => d.with_label(note),
                        };
                        let d = match needs {
                            Some((bound, at)) => {
                                d.with_secondary(at, format!("this conformance needs `{bound}`"))
                            }
                            None => d,
                        };
                        match extend {
                            Some(block) => d.with_help(format!("write `{block}`")),
                            None => match conform_help(add.as_ref(), make.as_ref()) {
                                Some(help) => d.with_help(help),
                                None => d,
                            },
                        }
                    }
                    NotConformForm::Any { origin } => {
                        match origin.map(|origin| origin.text(&format!("`any {interface}`"))) {
                            Some((None, text)) => d.with_label(text),
                            // No diagnostic points into the prelude, and this one has no note.
                            Some((Some(at), _)) if at.in_prelude() => d,
                            Some((Some(at), text)) => d.with_secondary(at, text),
                            None => d,
                        }
                    }
                    NotConformForm::Bound {
                        bound,
                        implied,
                        label,
                        make,
                        class_only,
                        add,
                        needs,
                    } => {
                        let d = if class_only {
                            d.with_label("only classes conform to `AnyObject`")
                        } else {
                            d
                        };
                        let d = match label {
                            Some(label) => d.with_label(label.text(&ty, &interface)),
                            None => d,
                        };
                        let d = match (bound, implied) {
                            (Some(bound), Some(Implied { written, base })) => d.with_secondary(
                                bound,
                                format!("`{written}` needs `{base}: {interface}`"),
                            ),
                            (Some(bound), None) => d.with_secondary(bound, "required by this bound"),
                            // A bound of the prelude has no span, so `implied` is not shown.
                            (None, _) => d,
                        };
                        let d = match needs {
                            Some((bound, at)) => {
                                d.with_secondary(at, format!("this conformance needs `{bound}`"))
                            }
                            None => d,
                        };
                        match conform_help(add.as_ref(), make.as_ref()) {
                            Some(help) => d.with_help(help),
                            None => d,
                        }
                    }
                }
            }
            DiagnosticKind::CannotExtend { written, at, form } => {
                let (message, remove) = match form {
                    CannotExtendForm::BuiltIn => (
                        format!("`{written}` is a built-in type; only the prelude extends it"),
                        None,
                    ),
                    CannotExtendForm::TypeParam => (
                        format!(
                            "`extend` takes a class, struct or enum, and `{written}` is a type parameter"
                        ),
                        None,
                    ),
                    CannotExtendForm::Unused { target, remove } => {
                        (format!("`{written}` does not appear in `{target}`"), remove)
                    }
                    CannotExtendForm::Associated => (
                        format!("an `extend` block cannot name an associated type in `{written}`"),
                        None,
                    ),
                    CannotExtendForm::Any => (
                        format!(
                            "`extend` takes a class, struct or enum, and `{written}` is an interface value"
                        ),
                        None,
                    ),
                };
                let d = Diagnostic::coded(code, at, message);
                match remove {
                    Some(span) => d.with_fix(Fix {
                        message: "remove the type parameters".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span,
                            replacement: " ".to_string(),
                        }],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::Ambiguous {
                method,
                receiver,
                interfaces,
                at,
                annotate,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!(
                        "`{method}` on `{receiver}` could come from {}",
                        listed(&interfaces, "or")
                    ),
                );
                match annotate {
                    Some(Annotate { call, binding, ty }) => {
                        d.with_help(format!("annotate the result: `let {binding}: {ty} = {call}`"))
                    }
                    None => d,
                }
            }
            DiagnosticKind::BoundTooWide {
                kind,
                written,
                limit,
                at,
            } => Diagnostic::coded(
                code,
                at,
                format!(
                    "the {} `{written}` gives more than {limit} inherited interfaces",
                    kind.noun()
                ),
            )
            .with_help(
                "an interface that inherits one interface twice, with different arguments, \
                 doubles this number at each level",
            ),
            DiagnosticKind::ClassOnly { ty, at, via } => {
                let d = Diagnostic::coded(code, at, "only classes conform to `AnyObject`")
                    .with_label(format!("`{ty}` is not a class"));
                match via {
                    Some((interface, at)) => {
                        d.with_secondary(at, format!("`{interface}` inherits `AnyObject`"))
                    }
                    None => d,
                }
            }
            DiagnosticKind::NoOperator {
                ty,
                op,
                at,
                leaf,
                form,
                needs,
            } => {
                let since = leaf.map(|leaf| format!(", since `{leaf}` has none"));
                let (message, label, help) = match form {
                    NoOperatorForm::Identity { make } => (
                        format!("`{ty}` has no `{op}`; classes compare by identity"),
                        format!("this has type `{ty}`"),
                        Some(format!("use `a is b`, or make `{make}` conform to `Eq`")),
                    ),
                    NoOperatorForm::Any { is } => (
                        format!("`{ty}` has no `{op}`"),
                        format!("this has type `{ty}`"),
                        is.then(|| "use `a is b`".to_string()),
                    ),
                    NoOperatorForm::Operator { make } => (
                        format!("`{ty}` has no `{op}`{}", since.unwrap_or_default()),
                        format!("this has type `{ty}`"),
                        conform_help(None, make.as_ref()),
                    ),
                };
                let d = Diagnostic::coded(code, at, message).with_label(label);
                let d = match needs {
                    Some((bound, at)) => {
                        d.with_secondary(at, format!("this conformance needs `{bound}`"))
                    }
                    None => d,
                };
                match help {
                    Some(help) => d.with_help(help),
                    None => d,
                }
            }
            DiagnosticKind::PayloadTooLarge {
                name,
                kind,
                instance,
                holds,
                holder,
                at,
                used,
            } => {
                let noun = kind.noun();
                // The span is the declaration, which holds the other types in an instance only.
                let this = if instance {
                    format!("an instance of this {noun}")
                } else {
                    format!("this {noun}")
                };
                let (message, label) = match (holder, kind) {
                    (Some(holder), _) => (
                        format!(
                            "{noun} `{name}` can make the payload of `{holder}` {PAYLOAD_LIMIT_GIB} GiB or more"
                        ),
                        Some(format!("`{holder}` holds {this} inline")),
                    ),
                    (None, payload_limit::Kind::Enum) => (
                        format!("the payload of enum `{name}` can be {PAYLOAD_LIMIT_GIB} GiB or more"),
                        held_label(&this, holds, "other enums", "structs"),
                    ),
                    (None, payload_limit::Kind::Struct) => (
                        format!("struct `{name}` can be {PAYLOAD_LIMIT_GIB} GiB or more"),
                        held_label(&this, holds, "enums", "other structs"),
                    ),
                };
                let d = Diagnostic::coded(code, at, message);
                let d = match label {
                    Some(label) => d.with_label(label),
                    None => d,
                };
                let d = match used {
                    Some(used) => d.with_secondary(used, "the instance is used here"),
                    None => d,
                };
                let whole = match kind {
                    payload_limit::Kind::Enum => "payload",
                    payload_limit::Kind::Struct => "struct",
                };
                d.with_help(format!(
                    "a {whole} must be less than {PAYLOAD_LIMIT_GIB} GiB. Hold a part in a class or an array. \
                     A class field adds the size of a pointer to the {whole}, and an array field adds \
                     the size of three pointers."
                ))
            }
            DiagnosticKind::NotAnInterface { written, at, kind } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{written}` is {}, not an interface", kind.named()),
                );
                match (kind, written.strip_prefix("any ")) {
                    (NotInterface::Any, Some(interface)) => {
                        d.with_help(format!("name the interface without `any`: `{interface}`"))
                    }
                    _ => d,
                }
            }
            DiagnosticKind::SelfOutside { at } => Diagnostic::coded(
                code,
                at,
                "`Self` is valid only inside a class, enum, interface or `extend` block",
            ),
            DiagnosticKind::InheritanceCycle { name, at, through } => {
                let message = if through.is_empty() {
                    format!("`{name}` inherits from itself")
                } else {
                    format!(
                        "`{name}` inherits from itself, through {}",
                        cycle_listed(&through)
                    )
                };
                Diagnostic::coded(code, at, message)
            }
            DiagnosticKind::AliasCycle {
                name,
                at,
                through,
                steps,
            } => {
                let message = if through.is_empty() {
                    format!("type alias `{name}` refers to itself")
                } else {
                    format!(
                        "type alias `{name}` refers to itself through {}",
                        cycle_listed(&through)
                    )
                };
                // The labels follow the names: a cut list shows the first steps only.
                let mut steps = steps;
                if cycle_shown(&through).1 > 0 {
                    steps.truncate(CYCLE_SHOWN);
                }
                let d = Diagnostic::coded(code, at, message)
                    .with_help("use an enum or a class for a type that holds itself");
                steps.into_iter().fold(d, |d, (from, to, span)| {
                    d.with_secondary(span, format!("`{from}` uses `{to}` here"))
                })
            }
            DiagnosticKind::AliasTooLarge {
                ty,
                at,
                limit,
                check,
            } => Diagnostic::coded(
                code,
                at,
                if check {
                    format!(
                        "with `{ty}`, the type aliases of this program have built more than {} distinct types",
                        thousands(limit)
                    )
                } else {
                    format!(
                        "the type that `{ty}` names holds more than {} distinct types",
                        thousands(limit)
                    )
                },
            )
            .with_help("hold an inner part in a field of a class, an enum or a struct"),
            DiagnosticKind::AnyForm { at, form } => match form {
                AnyFormKind::Optional {
                    written,
                    bare,
                    layers,
                } => {
                    let marks = "?".repeat(layers);
                    let value = if layers == 1 {
                        "an optional interface value".to_string()
                    } else {
                        format!("an interface value under {layers} optional layers")
                    };
                    Diagnostic::coded(code, at, format!("`{written}{marks}` needs parentheses"))
                        .with_help(format!("write `({written}){marks}` for {value}"))
                        .with_fix(Fix {
                            message: "add parentheses".to_string(),
                            safe: true,
                            edits: vec![
                                Edit {
                                    span: Span::new(bare.start(), bare.start()),
                                    replacement: "(".to_string(),
                                },
                                Edit {
                                    span: Span::new(bare.end(), bare.end()),
                                    replacement: ")".to_string(),
                                },
                            ],
                        })
                }
                AnyFormKind::Several { first, second } => {
                    Diagnostic::coded(code, at, "`any` takes one interface").with_help(format!(
                        "declare an interface that inherits `{first}` and `{second}`, and use it after `any`"
                    ))
                }
            },
            DiagnosticKind::MutMarkerMissing {
                at,
                name,
                callee,
                declared,
                fix,
            } => {
                let message = format!(
                    "the parameter `{name}` of `{callee}` is `mut`, so its argument needs `mut`"
                );
                let d = Diagnostic::coded(code, at, message).with_label("needs `mut`");
                let d = match declared {
                    Some(declared) => d.with_secondary(declared, "declared `mut` here"),
                    None => d,
                };
                match fix {
                    MissingFix::Add => d.with_fix(insert_mut("add `mut`", at)),
                    MissingFix::Let { name, arg } => d.with_help(format!(
                        "declare `{name}` with `let mut`, and pass `mut {arg}`"
                    )),
                    MissingFix::Field { name } => d.with_help(format!(
                        "declare the field `{name}` as `mut`, and pass it `mut`"
                    )),
                    MissingFix::Copy if declared.is_some() => d.with_help(format!(
                        "to change only a local copy, remove `mut` here and write \
                         `let mut {name} = {name}` in the body"
                    )),
                    MissingFix::NoFix | MissingFix::Copy => d,
                }
            }
            DiagnosticKind::MutMarkerNotMut { at, fix_end, form } => {
                let fix = Fix {
                    message: "remove `mut`".to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: Span::new(at.start(), fix_end),
                        replacement: String::new(),
                    }],
                };
                let d = match form {
                    NotMutForm::Written {
                        name,
                        callee,
                        declared,
                    } => {
                        let d = Diagnostic::coded(
                            code,
                            at,
                            format!("the parameter `{name}` of `{callee}` is not `mut`"),
                        )
                        .with_label("not a `mut` parameter");
                        match declared {
                            Some(declared) => {
                                d.with_secondary(declared, "declared without `mut` here")
                            }
                            None => d,
                        }
                    }
                    NotMutForm::Unnamed { position, callee } => Diagnostic::coded(
                        code,
                        at,
                        format!("argument {position} of `{callee}` is not `mut`"),
                    )
                    .with_label("not a `mut` parameter"),
                    NotMutForm::Class { shown } => Diagnostic::coded(
                        code,
                        at,
                        format!(
                            "`{shown}(…)` takes no `mut` argument: it copies each value into its \
                             field"
                        ),
                    )
                    .with_label("no `mut` here")
                    .with_help("a `mut` field can change after the object is made"),
                    NotMutForm::Variant { shown } => Diagnostic::coded(
                        code,
                        at,
                        format!(
                            "`{shown}(…)` takes no `mut` argument: it copies each value into its \
                             field"
                        ),
                    )
                    .with_label("no `mut` here")
                    .with_help("an enum value never changes after it is built"),
                };
                d.with_fix(fix)
            }
            DiagnosticKind::MutNotArgument {
                at,
                binding,
                fix_end,
            } => {
                let d = Diagnostic::coded(code, at, "`mut` marks an argument for a `mut` parameter")
                    .with_label("not an argument");
                let d = if binding {
                    d.with_help(
                        "a value is copied when it is bound; to change it, bind it with `let mut`",
                    )
                } else {
                    d
                };
                d.with_fix(Fix {
                    message: "remove `mut`".to_string(),
                    safe: true,
                    edits: vec![Edit {
                        span: Span::new(at.start(), fix_end),
                        replacement: String::new(),
                    }],
                })
            }
            DiagnosticKind::MutArgumentNotPlace { at, name, cause } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    "a `mut` argument must be a place: a variable, a `mut` field, or an element \
                     of one of these",
                );
                mut_not_place(d, &name, &cause)
            }
            DiagnosticKind::MutArgumentType {
                at,
                arg,
                name,
                position,
                callee,
                found,
                want,
                declared,
                declared_as,
                promotes,
                chain,
            } => {
                let param = match &name {
                    Some(name) => format!("the parameter `{name}` of `{callee}`"),
                    None => format!("argument {position} of `{callee}`"),
                };
                let (d, help) = if let Some(ChainForm { shown, label, help }) = chain {
                    let label = match label {
                        ChainLabel::WriteType | ChainLabel::Other => format!(
                            "this reads as `{found}`, but `{callee}` reads and writes back \
                             `{want}`"
                        ),
                        ChainLabel::WritesNone { field } => format!(
                            "`{callee}` can write `None` back, and the field `{field}` cannot \
                             hold `None`"
                        ),
                        ChainLabel::NoPlace { head } => {
                            let head = head.map_or_else(
                                || "the object".to_string(),
                                |head| format!("`{head}`"),
                            );
                            format!(
                                "{head} can be `None`, and then there is no place to store back to"
                            )
                        }
                    };
                    let message = match shown {
                        Some(shown) => {
                            format!("`{shown}` can be missing, so it cannot be a `mut` argument")
                        }
                        None => {
                            "a path through `?.` can be missing, so it cannot be a `mut` argument"
                                .to_string()
                        }
                    };
                    (Diagnostic::coded(code, at, message).with_label(label), help)
                } else {
                    let message = match name {
                        Some(_) => format!(
                            "the `mut` argument `{arg}` has type `{found}`, but {param} has \
                             type `{want}`"
                        ),
                        None => format!(
                            "{param} has type `{want}`, but its `mut` argument has type \
                             `{found}`"
                        ),
                    };
                    let note = if promotes {
                        format!(
                            "a `mut` argument must have exactly the parameter's type: the \
                             callee can store `None`, which {} `{found}` cannot hold",
                            indefinite(&found)
                        )
                    } else {
                        format!(
                            "a `mut` argument must have exactly the parameter's type: the \
                             callee stores {} `{want}` back into the place",
                            indefinite(&want)
                        )
                    };
                    let d = Diagnostic::coded(code, at, message)
                        .with_label(format!("has type `{found}`"));
                    (d, Some(note))
                };
                let d = match declared {
                    Some(declared) => {
                        d.with_secondary(declared, format!("declared `{declared_as}` here"))
                    }
                    None => d,
                };
                match help {
                    Some(help) => d.with_help(help),
                    None => d,
                }
            }
            DiagnosticKind::MutPlaceTwice {
                first,
                second,
                shown,
                inside,
            } => {
                let message = match inside {
                    Some(outer) => {
                        format!("`{shown}` is inside `{outer}`, which this call also passes `mut`")
                    }
                    None => format!(
                        "`{shown}` is passed `mut` twice: each `mut` argument needs its own place"
                    ),
                };
                Diagnostic::coded(code, second, message)
                    .with_label("passed again here")
                    .with_secondary(first, "first passed here")
            }
            DiagnosticKind::LetElseCanFinish { at } => Diagnostic::coded(
                code,
                at,
                "the `else` block of a `let ... else` must not finish",
            )
            .with_label("the block can finish here")
            .with_help("end the block with `return`, `break`, `continue` or `panic(...)`"),
            DiagnosticKind::LetElseValueEndsInBrace { at } => Diagnostic::coded(
                code,
                at,
                "the value of a `let ... else` cannot end in `}`",
            )
            .with_label("this ends in `}`")
            .with_fix(Fix {
                message: "put the value in `( )`".to_string(),
                safe: true,
                edits: vec![
                    Edit {
                        span: Span::new(at.start(), at.start()),
                        replacement: "(".to_string(),
                    },
                    Edit {
                        span: Span::new(at.end(), at.end()),
                        replacement: ")".to_string(),
                    },
                ],
            }),
            DiagnosticKind::RefutablePattern {
                at,
                shown,
                always: true,
            } => Diagnostic::coded(code, at, "a `let` without `else` takes only a name or `_`")
                .with_label(format!("`{shown}` always matches"))
                .with_help("use `match`"),
            DiagnosticKind::RefutablePattern {
                at,
                shown,
                always: false,
            } => Diagnostic::coded(code, at, "this pattern can fail to match")
                .with_label(format!("`{shown}` may not match"))
                .with_help("add an `else` block that does not finish, or use `if let` or `match`"),
            DiagnosticKind::StructFieldMut { keyword, remove } => {
                Diagnostic::coded(code, keyword, "a field of a struct has no `mut`")
                    .with_label("delete this word")
                    .with_help("a `let mut` holder can change each field")
                    .with_fix(Fix {
                        message: "remove the `mut`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: remove,
                            replacement: String::new(),
                        }],
                    })
            }
            DiagnosticKind::CannotBeAny {
                interface,
                owner,
                reason,
                at,
            } => {
                let inherited = owner.map(|owner| format!("it inherits `{owner}`"));
                let why = match (reason, inherited) {
                    (NotAny::TakesSelf { method }, None) => {
                        format!("`{method}` takes another `Self`")
                    }
                    (NotAny::TakesSelf { method }, Some(inherits)) => {
                        format!("{inherits}, whose `{method}` takes another `Self`")
                    }
                    (NotAny::Returns { method, ret }, None) => {
                        format!("`{method}` returns `{ret}`")
                    }
                    (NotAny::Returns { method, ret }, Some(inherits)) => {
                        format!("{inherits}, whose `{method}` returns `{ret}`")
                    }
                    (NotAny::Generic { method }, None) => format!("`{method}` is generic"),
                    (NotAny::Generic { method }, Some(inherits)) => {
                        format!("{inherits}, whose `{method}` is generic")
                    }
                    (NotAny::Assoc { owner, assoc }, None) => {
                        format!("`{owner}` has an associated type, `{assoc}`")
                    }
                    (NotAny::Assoc { assoc, .. }, Some(inherits)) => {
                        format!("{inherits}, which has an associated type, `{assoc}`")
                    }
                    (NotAny::InheritsSelf { inherited }, _) => {
                        format!("it inherits `{inherited}`, whose arguments name `Self`")
                    }
                };
                Diagnostic::coded(
                    code,
                    at,
                    format!("`{interface}` cannot be used as `any {interface}`: {why}"),
                )
                .with_label("used after `any` here")
                .with_help(format!("use a type parameter: `T: {interface}`"))
            }
            DiagnosticKind::TooManyInstances {
                name,
                at,
                count,
                limit,
            } => Diagnostic::coded(
                code,
                at,
                format!(
                    "the generic items of this program have more than {} instances",
                    thousands(limit)
                ),
            )
            .with_label(format!(
                "`{name}` has the most so far: {}",
                instances(count)
            ))
            .with_help(format!(
                "use fewer different type arguments, or put a non-generic item between `{name}` \
                 and the generic items that use it with different type arguments"
            )),
            DiagnosticKind::InheritedArgument {
                arg,
                interface,
                at,
            } => Diagnostic::coded(
                code,
                at,
                format!("`{arg}` cannot be an argument of an inherited interface"),
            )
            .with_help(format!(
                "write a type parameter of `{interface}` alone, or a type with no type \
                 parameter. With arguments built from type parameters, the inherited \
                 interfaces of a bound can double at each level of inheritance."
            )),
            DiagnosticKind::InheritedRequirement {
                name,
                interface,
                owner,
                at,
                first,
                assoc,
            } => Diagnostic::coded(
                code,
                at,
                format!(
                    "`{name}` is already {} of `{owner}`, which `{interface}` inherits",
                    if assoc {
                        "an associated type"
                    } else {
                        "a requirement"
                    }
                ),
            )
            .with_secondary(first, "first declared here"),
            DiagnosticKind::SelfNotFirst { at } => {
                Diagnostic::coded(code, at, "`self` is only the first parameter of a method")
            }
            DiagnosticKind::AssignToExpression { at, range } => match range {
                Some(range) => range.diagnostic(code, at),
                None => Diagnostic::coded(code, at, "cannot assign to this expression"),
            },
            DiagnosticKind::PushToTemporary { at, range: None } => {
                Diagnostic::coded(code, at, "cannot push to a temporary value")
                    .with_label("this value is not stored in a variable")
                    .with_help("store it in a `let mut` variable first")
            }
            DiagnosticKind::PushToTemporary {
                at,
                range: Some(range),
            } => {
                let d = Diagnostic::coded(code, at, "cannot push to a temporary value")
                    .with_label("a range index gives a new value");
                match range {
                    RangePush::Slice(range) => d.with_help(format!(
                        "keep the slice in a variable first: `let mut t = {range}`, then `t.push(...)`"
                    )),
                    RangePush::Element(target) => match target.help() {
                        Some(help) => d.with_help(help),
                        None => d,
                    },
                }
            }
            DiagnosticKind::RangeOutsideFor { at, form } => form.diagnostic(code, at),
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
                    BuiltIn::Interface => format!("`{name}` is a built-in interface"),
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
                Unbuilt::Chain {
                    links,
                    first_struct,
                } => {
                    let (shown, more) = cycle_shown(&links);
                    let mut parts: Vec<_> = (shown.iter())
                        .map(|(holder, held)| format!("`{holder}` holds a `{held}`"))
                        .collect();
                    if more > 0 {
                        parts.push(format!("{more} more"));
                    }
                    let chain = parts.join(", and ");
                    let d =
                        Diagnostic::coded(code, at, format!("no value of `{name}` can be built"))
                            .with_label(chain);
                    match first_struct {
                        Some(s) => d.with_help(format!(
                            "a field of type `{s}?`, or an enum with a variant that holds no `{s}`, ends the chain"
                        )),
                        None => d,
                    }
                }
            },
            DiagnosticKind::NoMain => {
                Diagnostic::coded(code, Span::new(0, 0), "no `main` function")
            }
            DiagnosticKind::MainSignature {
                at,
                form: MainForm::Signature,
            } => Diagnostic::coded(
                code,
                at,
                "`main` takes no parameters, or one `[string]`, and returns no value",
            ),
            DiagnosticKind::MainSignature {
                at,
                form: MainForm::TypeParams { remove },
            } => {
                let d = Diagnostic::coded(code, at, "`main` takes no type parameters");
                match remove {
                    Some(span) => d.with_fix(Fix {
                        message: "remove the type parameters".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span,
                            replacement: String::new(),
                        }],
                    }),
                    None => d,
                }
            }
            DiagnosticKind::UnknownType { name, at, form } => match form {
                UnknownForm::Name { suggestion } => similar(
                    Diagnostic::coded(code, at, format!("cannot find type `{name}`")).with_help(
                        "the types are `i64`, `u8`, `f64`, `bool`, `char`, `string`, classes, enums, structs and arrays such as `[i64]`",
                    ),
                    at,
                    suggestion,
                ),
                UnknownForm::NoAssociated { base, add } => {
                    let d = Diagnostic::coded(
                        code,
                        at,
                        format!("`{base}` has no associated type `{name}`"),
                    );
                    match add {
                        Some(add) => d.with_help(add.help()),
                        None => d,
                    }
                }
                UnknownForm::OfAssociatedInAlias { written, base } => Diagnostic::coded(
                    code,
                    at,
                    format!("a type alias cannot name `{written}`"),
                )
                .with_help(format!(
                    "give the alias a parameter that stands for `{base}`"
                )),
                UnknownForm::ConcreteBase { own } => {
                    let d = Diagnostic::coded(
                        code,
                        at,
                        "an associated type is written through a type parameter, as `S.Item` for `S: Source`",
                    );
                    if own {
                        d.with_help(format!("write `{name}`"))
                    } else {
                        d
                    }
                }
                UnknownForm::ChoiceNamesItself { shadows } => Diagnostic::coded(
                    code,
                    at,
                    format!("an associated type's choice cannot name `{name}`"),
                )
                .with_help(if shadows {
                    format!("`{name}` also names a type, which this associated type hides: rename one of them")
                } else {
                    "write the type itself".to_string()
                }),
                UnknownForm::ChoiceLoops => Diagnostic::coded(
                    code,
                    at,
                    format!("the associated type `{name}` needs itself to find its type"),
                )
                .with_help("write a choice that does not lead back to the same associated type"),
                UnknownForm::ChoiceTooDeep => Diagnostic::coded(
                    code,
                    at,
                    format!("the associated type `{name}` needs more than {CHOICE_DEPTH_LIMIT} steps to find its type"),
                )
                .with_help("write a choice that gives its type in fewer steps, or that does not grow its types"),
                UnknownForm::BoundsLoop { base } => Diagnostic::coded(
                    code,
                    at,
                    format!("`{base}.{name}` is not known here: a bound of `{base}` waits for this bound"),
                )
                .with_help("write one of the bounds that wait for each other without an associated type"),
            },
            DiagnosticKind::BodyHasNoValue { function, want, at } => Diagnostic::coded(
                code,
                at,
                format!("function `{function}` must return `{want}`, but its body has no value"),
            ),
            DiagnosticKind::Mismatch {
                want,
                got,
                at,
                origin,
                fix,
                conversion,
                layers,
            } => {
                let want = want.text();
                let mut d =
                    Diagnostic::coded(code, at, format!("expected {want}, found `{got}`"));
                // A rule labels the mistake. No diagnostic points into the prelude, so a fact
                // there is a note.
                d = match origin.map(|origin| origin.text(&want)) {
                    Some((None, text)) => d.with_label(text),
                    Some((Some(at), text)) if at.in_prelude() => d.with_note(text),
                    Some((Some(at), text)) => d.with_secondary(at, text),
                    None => d,
                };
                if conversion {
                    d = d.with_help(
                        "a conversion that can fail gives an optional: give a default with `??`, or unwrap it with `if let`",
                    );
                }
                if layers {
                    d = d.with_help(
                        "a value converts through one `?` only: wrap it in `Some(...)` for each further layer",
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
                                replacement: through.map_or_else(|| format!("{to}("), |through| {
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
                wrap,
            } => similar_label(
                Diagnostic::coded(code, at, format!("cannot find binding `{name}`")),
                at,
                suggestion,
                wrap,
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
            DiagnosticKind::BareBreak { at, valued } => Diagnostic::coded(
                code,
                at,
                "`break` has no value, but this `loop` gives one",
            )
            .with_secondary(valued, "this `break` gives the loop a value")
            .with_help("give every `break` of this `loop` a value, or none"),
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
            DiagnosticKind::IsNeedsObjects {
                found,
                at,
                help,
                add,
                peel,
            } => {
                let d = Diagnostic::coded(code, at, format!("`is` needs objects; found `{found}`"));
                if let Some(PeelIs { text, layers }) = peel {
                    let layers = match layers {
                        PeelLayers::One => "one layer",
                        PeelLayers::OfEachSide => "one layer of each side",
                        PeelLayers::Extra => "the extra layers",
                    };
                    d.with_help(format!("peel {layers} first: `{text}`"))
                } else if let Some(add) = add {
                    d.with_help(add.help())
                } else if help {
                    d.with_help("use `==` to compare values")
                } else {
                    d
                }
            }
            DiagnosticKind::AnyMayNotHold { found, at, make } => {
                let d = Diagnostic::coded(code, at, format!("`{found}` may not hold an object"));
                match make {
                    Some(make) => d.with_help(format!(
                        "make `{make}` inherit `AnyObject`: `interface {make}: AnyObject`"
                    )),
                    None => d,
                }
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
            DiagnosticKind::CannotInfer {
                literal,
                binding,
                ty,
                at,
                regroup,
            } => {
                let (literal, example) = match (literal, binding) {
                    (Literal::None, name) => (
                        "None",
                        format!(
                            "let {}: {} = None",
                            name.as_deref().unwrap_or("x"),
                            ty.as_deref().unwrap_or("...?")
                        ),
                    ),
                    (Literal::EmptyArray, name) => (
                        "[]",
                        format!(
                            "let {}: {} = []",
                            name.as_deref().unwrap_or("xs"),
                            ty.as_deref().unwrap_or("[...]")
                        ),
                    ),
                };
                let d = Diagnostic::coded(code, at, format!("cannot infer the type of `{literal}`"));
                match regroup {
                    Some(start) => d.with_fix(Fix {
                        message: "`??` groups to the right; to unwrap the left side twice, use parentheses"
                            .to_string(),
                        safe: false,
                        edits: vec![
                            Edit {
                                span: Span::new(start, start),
                                replacement: "(".to_string(),
                            },
                            Edit {
                                span: Span::new(at.end(), at.end()),
                                replacement: ")".to_string(),
                            },
                        ],
                    }),
                    None => d.with_help(format!("write the type: `{example}`")),
                }
            }
            DiagnosticKind::CannotInferParam {
                param,
                callee,
                example,
                at,
                in_result,
            } => {
                let annotate = if in_result {
                    "annotate the result, or "
                } else {
                    ""
                };
                Diagnostic::coded(code, at, format!("cannot infer `{param}` for `{callee}`"))
                    .with_help(format!("{annotate}write the type: `{example}`"))
            }
            DiagnosticKind::CannotInferSome { callee, at } => {
                Diagnostic::coded(code, at, format!("cannot infer `T` for `{callee}`"))
                    .with_help("annotate the optional's type, as in `let x: i64?? = Some(None)`")
            }
            DiagnosticKind::CannotInferResult { param, variant, at } => Diagnostic::coded(
                code,
                at,
                format!("cannot infer `{param}` for `{variant}`"),
            )
            .with_help("annotate the result's type, as in `let r: Result<i64, string> = Ok(...)`"),
            DiagnosticKind::UnwrapNeedsOptional {
                keyword,
                found,
                at,
                compare,
            } => {
                // A plain `let x = 5` is right, so the message names the pattern.
                let subject = match keyword {
                    Unwrap::Let => "a `let` pattern".to_string(),
                    _ => format!("`{}`", keyword.word()),
                };
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("{subject} needs an enum or an optional; this is `{found}`"),
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
            DiagnosticKind::NotAFunction { name, what, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is {what}, not a function"))
            }
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
                grouped,
                labeled,
                head,
            } => {
                let edit = |span: Span, replacement: &str| Edit {
                    span,
                    replacement: replacement.to_string(),
                };
                // A grouped value keeps the `)` after it, so only `T(` becomes its `(`, and
                // each comment before the value stays: parentheses around the call stay paired.
                // Otherwise the head and the tail go, with those parentheses, in pairs.
                let edits = if grouped {
                    part_edits(&head, "(")
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
                    message: "remove the conversion".to_string(),
                    safe: true,
                    edits,
                })
            }
            DiagnosticKind::LabeledArgument { at, variant: None } => {
                Diagnostic::coded(code, at, "this call takes no labels on its arguments").with_help(
                    "remove the labels: only a constructor, or a variant that declares labels, takes labeled arguments",
                )
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
                problem,
                at,
            } => Diagnostic::coded(
                code,
                at,
                match problem {
                    PrintProblem::Count => format!("`{function}` takes one value"),
                    PrintProblem::NoValue => format!("the argument of `{function}` has no value"),
                },
            ),
            DiagnosticKind::PrintNever { function, at } => Diagnostic::coded(
                code,
                at,
                format!("`{function}` never runs: its argument never finishes"),
            ),
            DiagnosticKind::MayBeNone {
                at,
                peel: Some(Peel {
                    receiver,
                    left,
                    member,
                    layers,
                    help,
                }),
                ..
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("`{receiver}?.` leaves a `{left}`, which may be `None`"),
                );
                if help {
                    d.with_help(peel_text("other", &receiver, &member, layers))
                } else {
                    d
                }
            }
            DiagnosticKind::MayBeNone {
                value,
                at,
                access,
                write,
                unwrap,
                ..
            } => {
                let d = Diagnostic::coded(code, at, format!("{} may be `None`", named(value)));
                let d = match unwrap_text(unwrap, access.is_some()) {
                    Some(help) => d.with_help(help),
                    None => d,
                };
                match access {
                    Some(access) => {
                        let fix = if write {
                            "to do nothing when it is `None`, use `?.`"
                        } else {
                            "if a `None` result is fine, use `?.`"
                        };
                        d.with_fix(unsafe_fix(fix, access, "?."))
                    }
                    None => d,
                }
            }
            DiagnosticKind::ChainWriteOnStruct { at, replace } => {
                let d = Diagnostic::coded(code, at, "`?.` cannot write a field of a struct value yet");
                match replace {
                    Some(base) => d.with_help(replace_text(&base)),
                    None => d,
                }
            }
            DiagnosticKind::NoMember {
                owner,
                kind,
                name,
                at,
                suggestion,
                add,
            } => {
                let d = similar(
                    Diagnostic::coded(
                        code,
                        at,
                        format!("`{owner}` has no {} `{name}`", kind.word()),
                    ),
                    at,
                    suggestion,
                );
                match add {
                    Some(add) => d.with_help(add.help()),
                    None => d,
                }
            }
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
            DiagnosticKind::SameObject {
                class,
                at,
                param: false,
                any,
            } => {
                // An `any` type needs parentheses before `?`.
                let optional = if any {
                    format!("({class})?")
                } else {
                    format!("{class}?")
                };
                Diagnostic::coded(
                    code,
                    at,
                    format!("every element would be the same `{class}` object"),
                )
                .with_help(format!(
                    "start from `[None; n]` with type `[{optional}]` and fill it in a loop"
                ))
            }
            DiagnosticKind::SameObject {
                class,
                at,
                param: true,
                any,
            } => Diagnostic::coded(
                code,
                at,
                format!("every element could be the same `{class}` object"),
            )
            .with_label(if any {
                format!("`{class}` may hold an object, and every element would be the same object")
            } else {
                format!("`{class}` may be a class, and every element would be the same object")
            }),
            DiagnosticKind::CannotIndex { ty, at, range } => {
                Diagnostic::coded(code, at, format!("cannot index `{ty}`")).with_help(if range {
                    "only arrays and strings take a range index"
                } else {
                    "only arrays can be indexed"
                })
            }
            DiagnosticKind::StringIndex { at, base, usage } => {
                // `string` takes no `[u8]`, so a write has no bytes form. A base with no one-line
                // text has no code to repeat, so the help states the steps.
                let help = match (base, usage) {
                    (Some(base), usage) => {
                        let [bs, cs, i] = string_index_names(&base);
                        match usage {
                            IndexUsage::Read => format!(
                                "for bytes, write `let {bs} = {base}.bytes()` once, then `{bs}[{i}]`; for characters, write `let {cs} = {base}.chars()` once, then `{cs}[{i}]`"
                            ),
                            IndexUsage::Write => format!(
                                "to change a character, write `let mut {cs} = {base}.chars()`, change `{cs}[{i}]`, then write `{base} = string({cs})`"
                            ),
                        }
                    }
                    (None, IndexUsage::Read) => "for bytes, put `bytes()` of the string in a `let`, then index it; for characters, do the same with `chars()`".to_owned(),
                    (None, IndexUsage::Write) => "to change a character, put `chars()` of the string in a `let mut`, change it, then write `string(...)` back".to_owned(),
                };
                Diagnostic::coded(code, at, "a string has no single index")
                    .with_label("this is a string")
                    .with_help(help)
            }
            DiagnosticKind::StringGetNeedsRange { at, base, call } => {
                // A call after a failed `?.` fails again, so its help names the range only.
                let example = match (call, base) {
                    (GetCall::Plain, Some(base)) => format!("{base}.get(1..3)"),
                    (GetCall::Safe, Some(base)) => format!("{base}?.get(1..3)"),
                    (GetCall::SafeFailed | GetCall::Payload, _)
                    | (GetCall::Plain | GetCall::Safe, None) => "1..3".to_owned(),
                };
                Diagnostic::coded(code, at, "a string's `get` takes a range")
                    .with_label("this is not a range")
                    .with_help(format!("write a range, such as `{example}`"))
            }
            DiagnosticKind::ArrayGetTakesIndex { at, base } => {
                // With no `base`, a `?.` or an optional touched the array, and the program has no
                // name to write: `base[a..b]` on an optional array is E0062, and Sisu has no `?[`.
                let range = match base {
                    Some(base) => format!("use `{base}[a..b]`"),
                    None => "index the array with `[a..b]`".to_owned(),
                };
                Diagnostic::coded(code, at, "an array's `get` takes an `i64`, not a range")
                    .with_label("this is a range")
                    .with_help(format!(
                        "{range}, which stops the program when the range is out of bounds"
                    ))
            }
            DiagnosticKind::SumNeedsNumbers {
                method,
                ty,
                targets,
                at,
                function,
            } => {
                let targets = one_of(&targets);
                let d =
                    Diagnostic::coded(code, at, format!("`{method}` needs {targets}, found `{ty}`"));
                if function {
                    d.with_label(format!("`{method}` is a method of {targets}, not a function"))
                } else {
                    d
                }
            }
            DiagnosticKind::MethodAsFunction {
                name,
                at,
                receiver,
                bound: Some(bound),
            } => {
                let d = Diagnostic::coded(code, at, format!("`{name}` is a method, not a function"));
                // The label always has a `need`, so `Holds`, which reads the type, does not occur.
                let d = d.with_label(bound.label.text("", &bound.interface));
                let call = match receiver {
                    MethodReceiver::SelfValue { .. } => {
                        format!("call `{name}` as a method of `self`")
                    }
                    MethodReceiver::Optional { .. } => format!("call `{name}` with `?.`"),
                    MethodReceiver::Argument { .. } => format!("call `{name}` as a method"),
                };
                let fix = if bound.takes { "" } else { " and fix its arguments" };
                match conform_help(bound.add.as_ref(), bound.make.as_ref()) {
                    Some(help) => d.with_help(format!("{help}, then {call}{fix}")),
                    None => d,
                }
            }
            DiagnosticKind::MethodAsFunction {
                name,
                at,
                receiver,
                bound: None,
            } => {
                let d = Diagnostic::coded(code, at, format!("`{name}` is a method, not a function"));
                let (message, safe, edits) = match receiver {
                    // The first argument was checked without its parameter's type, so a correct
                    // call can be unsafe: the message does not say that the arguments are wrong.
                    MethodReceiver::SelfValue { safe } => (
                        if safe {
                            format!("call `{name}` as a method of `self`")
                        } else {
                            format!("try calling `{name}` as a method of `self`")
                        },
                        safe,
                        vec![Edit {
                            span: Span::new(at.start(), at.start()),
                            replacement: "self.".to_string(),
                        }],
                    ),
                    MethodReceiver::Argument { argument, safe } => (
                        method_form_message(&name, safe),
                        safe,
                        argument.edits(&format!(".{name}(")),
                    ),
                    MethodReceiver::Optional {
                        argument,
                        value,
                        takes,
                    } => {
                        let value = named(value);
                        let d = d.with_help(format!(
                            "{value} may be `None`: to call `{name}` only when it is present, unwrap it with `if let`"
                        ));
                        let fix = if takes { "" } else { ", then fix its arguments" };
                        return d.with_fix(Fix {
                            message: format!(
                                "to skip the call when {value} is `None`, call `{name}` with `?.`{fix}"
                            ),
                            safe: false,
                            edits: argument.edits(&format!("?.{name}(")),
                        });
                    }
                };
                d.with_fix(Fix {
                    message,
                    safe,
                    edits,
                })
            }
            DiagnosticKind::NestingTooDeep {
                at,
                limit,
                form: NestingForm::Tree,
            } => Diagnostic::coded(code, at, "nesting is too deep")
                .with_help(format!("the limit is {limit} levels")),
            DiagnosticKind::NestingTooDeep {
                at,
                limit,
                form: NestingForm::Chain,
            } => Diagnostic::coded(code, at, format!("this chain makes the nesting deeper than {limit} levels"))
                .with_label("each operator, call, field or index in a chain adds one level")
                .with_help("put a part of the chain in a `let`"),
            DiagnosticKind::NestingTooDeep {
                at,
                limit,
                form: NestingForm::ElseIf,
            } => Diagnostic::coded(
                code,
                at,
                format!("this `else if` chain makes the nesting deeper than {limit} levels"),
            )
            .with_label("each `else if` adds one level")
            .with_help("write the branches as `match` arms with `if` guards"),
            DiagnosticKind::NestingTooDeep {
                at,
                limit,
                form: NestingForm::Alias(ty),
            } => Diagnostic::coded(
                code,
                at,
                format!("the type that `{ty}` names nests deeper than {limit} levels"),
            )
            .with_help("hold an inner part in a field of a class, an enum or a struct"),
            DiagnosticKind::InvalidUtf8 { at } => {
                Diagnostic::coded(code, at, "source is not valid UTF-8")
                    .with_help("save the file as UTF-8")
            }
            DiagnosticKind::AssignToSelf { at, struct_self } => {
                Diagnostic::coded(code, at, "cannot assign to `self`").with_help(if struct_self {
                    STRUCT_SELF_HELP
                } else {
                    "an object is shared: `self` can already write every `mut` field"
                })
            }
            DiagnosticKind::AssignToFunction { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is a function, not a variable"))
            }
            DiagnosticKind::MutOnUnderscore { at, remove } => {
                Diagnostic::coded(code, at, "`_` binds nothing, so it cannot be `mut`").with_fix(
                    Fix {
                        message: "use `let _`".to_string(),
                        safe: true,
                        edits: vec![Edit {
                            span: remove,
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
            DiagnosticKind::CannotInterpolate { ty, at } => {
                Diagnostic::coded(code, at, format!("cannot interpolate `{ty}`"))
            }
            DiagnosticKind::PrintsOptional { value, at } => {
                let (subject, call) = match value {
                    Some(path) => (format!("`{path}`"), format!("`{path}.to_string()`")),
                    None => ("this value".to_string(), "`.to_string()` on the value".to_string()),
                };
                Diagnostic::coded(
                    code,
                    at,
                    format!("{subject} can be `None`, and then this writes the word \"None\""),
                )
                .with_help(format!(
                    "use `if let`, give a default with `??`, or call {call} to write \"None\" on purpose"
                ))
            }
            DiagnosticKind::WriteToCopy {
                place,
                root,
                at,
                copy,
                help,
            } => {
                let (of, name) = match place {
                    Some(place) => (format!("`{place}`"), place),
                    None => (format!("a part of `{root}`"), root),
                };
                Diagnostic::coded(
                    code,
                    at,
                    format!("this writes to a copy of {of}, and nothing reads the copy"),
                )
                .with_label(format!("`{name}` does not change"))
                .with_secondary(copy, "a copy")
                .with_help(help)
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
            DiagnosticKind::TypeAsValue { name, at, alias: None } => {
                Diagnostic::coded(code, at, format!("`{name}` is a type, not a value"))
            }
            DiagnosticKind::TypeAsValue { name, at, alias: Some(ty) } => Diagnostic::coded(
                code,
                at,
                format!("`{name}` is a type alias of `{ty}`, which has no constructor"),
            ),
            DiagnosticKind::InterfaceAsValue { name, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is an interface, not a value"))
            }
            DiagnosticKind::ParamAsValue { name, what, at } => {
                Diagnostic::coded(code, at, format!("`{name}` is {what}, not a value"))
            }
            DiagnosticKind::EnumAsValue { name, variant, at } => {
                let d = Diagnostic::coded(code, at, format!("`{name}` is an enum, not a value"));
                match variant {
                    Some(variant) => d.with_help(format!("write a variant, such as `{variant}`")),
                    None => d,
                }
            }
            DiagnosticKind::FunctionAsValue {
                name,
                at,
                takes_arguments,
            } => {
                let d = Diagnostic::coded(code, at, format!("`{name}` is a function, not a value"));
                d.with_help(if takes_arguments {
                    "call it with its arguments".to_string()
                } else {
                    format!("call it: `{name}()`")
                })
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
                    } => (form, receiver.edits(&format!(".{name}("))),
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
                Diagnostic::coded(code, at, "a format spec is `#` or `[<^>][width][.precision]`")
            }
            DiagnosticKind::UnsupportedFormatSpec { at, form: SpecForm::Plain } => {
                Diagnostic::coded(
                    code,
                    at,
                    "Sisu's format specs take `#`, or an alignment, a width and a precision",
                )
            }
            DiagnosticKind::UnsupportedFormatSpec { at, form: SpecForm::Pretty(limit) } => {
                Diagnostic::coded(
                    code,
                    at,
                    "`#` does not combine with a width, alignment or precision",
                )
                .with_help(match limit {
                    None => "remove the `#`, or the width, alignment and precision",
                    Some(SpecLimit::Width) => {
                        "remove the width, alignment and precision, or remove the `#` and limit the width to 1000"
                    }
                    Some(SpecLimit::Precision) => {
                        "remove the width, alignment and precision, or remove the `#` and limit the precision to 30"
                    }
                    Some(SpecLimit::Both) => {
                        "remove the width, alignment and precision, or remove the `#` and limit the width to 1000 and precision to 30"
                    }
                })
            }
            DiagnosticKind::UnsupportedFormatSpec { at, form: SpecForm::Debug } => {
                Diagnostic::coded(code, at, "Sisu's format specs have no `?`")
                    .with_help("Sisu shows a value with its `Display`")
                    .with_fix(safe_fix("delete the `:?`", at, ""))
            }
            DiagnosticKind::UnsupportedFormatSpec { at, form: SpecForm::DebugSpec } => {
                Diagnostic::coded(code, at, "Sisu's format specs have no `?`")
                    .with_help("Sisu shows a value with its `Display`")
                    .with_fix(safe_fix("delete the `?`", last_byte(at), ""))
            }
            DiagnosticKind::UnsupportedFormatSpec { at, form: SpecForm::PrettyDebug } => {
                Diagnostic::coded(code, at, "Sisu's format specs have no `#?`").with_fix(
                    safe_fix(
                        "delete the `?`: `#` alone lays the value out over lines",
                        last_byte(at),
                        "",
                    ),
                )
            }
            DiagnosticKind::FormatSpecLimit { at, width } => Diagnostic::coded(
                code,
                at,
                if width {
                    "a width is at most 1000"
                } else {
                    "a precision is at most 30"
                },
            ),
            DiagnosticKind::PrecisionNeedsF64 { ty, at, help } => {
                let d = Diagnostic::coded(code, at, "precision needs an `f64`")
                    .with_label(format!("the value is `{ty}`"));
                match help {
                    Some(PrecisionHelp::Default { part, spec }) => d.with_help(format!(
                        "give a default with `??` first, for example `{{{part} ?? 0.0{spec}}}`"
                    )),
                    Some(PrecisionHelp::Convert(operand)) => d.with_fix(Fix {
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
            DiagnosticKind::EnumNamedLocal { name, at, optional } => Diagnostic::coded(
                code,
                at,
                format!(
                    "`{name}` names {}",
                    if optional { "an optional type" } else { "an enum" }
                ),
            ),
            DiagnosticKind::BareVariant {
                name,
                enums,
                labelled,
                at,
            } => {
                // The whole span is replaced, so parentheses around the name go. The fix is certain
                // only when one enum has the variant.
                let safe = enums.len() == 1;
                let qualify = |e: &String| Fix {
                    message: format!("write `{e}.{name}`"),
                    safe,
                    edits: vec![Edit {
                        span: at,
                        replacement: format!("{e}.{name}"),
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
                // A fix that writes a loop label's name after `break` does not parse.
                enums
                    .iter()
                    .filter(|e| !labelled.contains(e))
                    .fold(d, |d, e| d.with_fix(qualify(e)))
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
                type_name,
                method,
                at,
                receiver,
            } => {
                let message = if method {
                    format!("`{name}` is a method of `{type_name}`; call it on a value")
                } else {
                    format!("`{name}` is a variant; build it with `{type_name}.{name}`")
                };
                let d = Diagnostic::coded(code, at, message);
                match receiver {
                    Some(TypeReceiver {
                        argument,
                        takes,
                        safe,
                    }) => d.with_fix(Fix {
                        message: method_form_message(&name, takes),
                        safe,
                        edits: argument.edits(&format!(".{name}(")),
                    }),
                    None => d,
                }
            }
            DiagnosticKind::EnumField { enum_name, at } => Diagnostic::coded(
                code,
                at,
                format!("`{enum_name}` has no fields; take it apart with `match`"),
            ),
            DiagnosticKind::MatchIdiom { form, at } => match form {
                MatchIdiomForm::Switch => Diagnostic::coded(code, at, "Sisu writes `match`")
                    .with_fix(unsafe_fix("write `match`", at, "match")),
                MatchIdiomForm::Case { remove } => {
                    let d = Diagnostic::coded(code, at, "an arm is `pattern => body`, with no `case`");
                    match remove {
                        Some(remove) => d.with_fix(Fix {
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
                        None => d,
                    }
                }
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
                ForeignPattern::Labels { form, labels } => {
                    let d = Diagnostic::coded(
                        code,
                        at,
                        format!("a pattern binds by position: `{form}`"),
                    );
                    match labels {
                        Some(labels) => d.with_fix(Fix {
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
                        None => d,
                    }
                }
                ForeignPattern::Braces { form, fix } => {
                    let message = format!("write `{form}`");
                    let found = Diagnostic::coded(code, at, message.as_str());
                    match fix {
                        Some((replace, fields)) => {
                            found.with_fix(unsafe_fix(&message, replace, &fields))
                        }
                        None => found,
                    }
                }
                ForeignPattern::TypeArgs { fix } => {
                    let d = Diagnostic::coded(code, at, "a pattern takes no type arguments")
                        .with_help("the type of the matched value gives the type arguments");
                    if fix {
                        d.with_fix(safe_fix("remove the type arguments", at, ""))
                    } else {
                        d
                    }
                }
            },
            DiagnosticKind::CannotMatch { ty, at } => {
                Diagnostic::coded(code, at, format!("cannot `match` on `{ty}`"))
                    .with_help("`match` takes enums, optionals, integers, `bool`, `char` and `string`")
            }
            DiagnosticKind::NotVariantHere { name, lower, at } => {
                let d = Diagnostic::coded(code, at, format!("`{name}` is not a variant here"));
                let help = "a binding starts with a lower-case letter";
                match lower {
                    LowerName::Binding(lower) => d
                        .with_help(help)
                        .with_fix(unsafe_fix(&format!("write `{lower}`"), at, &lower)),
                    LowerName::Keyword(lower) => d.with_help(format!(
                        "{help}, but `{lower}` is a keyword; use a different name"
                    )),
                    LowerName::Reserved(lower) => d.with_help(format!(
                        "{help}, but `{lower}` is reserved; use a different name"
                    )),
                }
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
                let (pattern, message) = if gap || missing == "_" {
                    ("_", "add a `_ =>` arm".to_string())
                } else {
                    (missing.as_str(), format!("add an arm for `{missing}`"))
                };
                let arm = match close {
                    Some(close) => format!("\n{indent}{pattern} => todo\n{close}"),
                    None => format!("{indent}{pattern} => todo\n"),
                };
                d.with_fix(unsafe_fix(&message, insert, &arm))
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
                .with_fix(Fix {
                    message: format!("remove the {what}"),
                    safe: false,
                    edits: part_edits(&remove, ""),
                })
            }
            DiagnosticKind::RedundantSemicolon { at, remove, space } => {
                let warning = Diagnostic::coded(code, at, "unnecessary `;`");
                match remove {
                    Some(remove) => warning.with_fix(safe_fix(
                        "remove the `;`",
                        remove,
                        if space { " " } else { "" },
                    )),
                    None => warning,
                }
            }
            DiagnosticKind::AlwaysMatches { keyword, at, plain } => match keyword {
                Unwrap::IfLet | Unwrap::LetElse => {
                    let d = Diagnostic::coded(
                        code,
                        at,
                        format!("`{}` always matches here", keyword.word()),
                    );
                    match plain {
                        Plain::No => d.with_help("use `match`"),
                        Plain::Let => d.with_help("use a plain `let`"),
                        Plain::RemoveElse { span, safe } => {
                            let message = "remove the `else` block";
                            d.with_fix(if safe {
                                safe_fix(message, span, "")
                            } else {
                                unsafe_fix(message, span, "")
                            })
                        }
                    }
                }
                Unwrap::WhileLet => Diagnostic::coded(
                    code,
                    at,
                    "`while let` always matches here, so the pattern never ends the loop",
                ),
                Unwrap::Let => unreachable!("a `let` with no `else` reports E0174, not E0131"),
            },
            DiagnosticKind::ForeignRange { form, at } => match form {
                ForeignRange::Exclusive { start, end, remove } => {
                    let start = start.as_deref().unwrap_or("start");
                    let end = end.as_deref().unwrap_or("end");
                    let d = Diagnostic::coded(code, at, format!("write `{start}..{end}`"));
                    match remove {
                        Some(lt) => d.with_fix(safe_fix("remove the `<`", lt, "")),
                        None => d,
                    }
                }
                ForeignRange::Colon {
                    base,
                    base_place,
                    start,
                    end,
                    whole,
                } => {
                    let negative = [&start, &end]
                        .into_iter()
                        .flatten()
                        .any(|b| matches!(b, ColonBound::Negative));
                    if negative {
                        // No fix and no rewrite: the parser cannot see the type of `base`, which
                        // counts bytes for a string, and a bound can change `base`.
                        let head = "Sisu has no negative index";
                        let message = if base_place {
                            format!("{head}: count from `{base}.len()`")
                        } else {
                            head.to_string()
                        };
                        Diagnostic::coded(code, at, message)
                    } else {
                        let text = |b: Option<ColonBound>| match b {
                            Some(ColonBound::Plain(text)) => text,
                            _ => String::new(),
                        };
                        let written = match (start, end) {
                            (None, None) => base,
                            (start, end) => format!("{base}[{}..{}]", text(start), text(end)),
                        };
                        let d = Diagnostic::coded(code, at, format!("write `{written}`"));
                        match whole {
                            Some(whole) => d.with_fix(safe_fix("remove the `[:]`", whole, "")),
                            // The parser cannot see the type of `base`: on a string, `..` counts
                            // bytes where Python counts characters; a bound past the end stops
                            // the program where Python stops at the end.
                            None => d.with_fix(unsafe_fix("write `..`", at, "..")),
                        }
                    }
                }
                ForeignRange::Ellipsis { start, end } => {
                    let start = start.as_deref().unwrap_or("start");
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
            DiagnosticKind::RangeBoundMissing {
                start,
                inclusive,
                end,
                at,
            } => {
                let start = start.as_deref().unwrap_or("start");
                let end = end.as_deref().unwrap_or("end");
                let op = if inclusive { "..=" } else { ".." };
                Diagnostic::coded(code, at, "a range pattern needs a start and an end")
                    .with_help(format!("write `{start}{op}{end}`"))
            }
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
            DiagnosticKind::BreakValueNotLoop {
                keyword,
                at,
                loop_at,
            } => Diagnostic::coded(code, at, format!("`break` with a value in a `{keyword}`"))
                .with_secondary(loop_at, format!("this `{keyword}` has no value"))
                .with_help(
                    "only `loop` gives a value through `break`; assign to a variable declared before the loop instead",
                ),
            DiagnosticKind::LabelClash {
                name,
                at,
                declared,
                form,
            } => {
                let (message, span, help) = match form {
                    ClashForm::Name { kind, .. } => (
                        format!(
                            "loop label `{name}` has the same name as the {} `{name}`",
                            kind.word()
                        ),
                        at,
                        "rename the loop label".to_string(),
                    ),
                    ClashForm::Operator { next } => (
                        format!("`{name}` is a loop label here, so it cannot start a value"),
                        at.to(next),
                        format!(
                            "the value of `break {name}` comes after the loop label; if `{name}` also names a value, rename the loop label"
                        ),
                    ),
                    ClashForm::Call { bracket, ref text } => (
                        format!("`{name}` is a loop label here, so `(` cannot call it"),
                        at.to(bracket),
                        format!(
                            "to give a value in parentheses, put a space after the loop label: `break {name} {text}`; if `{name}` also names a function, rename the loop label"
                        ),
                    ),
                    ClashForm::Index { bracket, ref text } => (
                        format!("`{name}` is a loop label here, so `[` cannot index it"),
                        at.to(bracket),
                        format!(
                            "to give an array as the value, put a space after the loop label: `break {name} {text}`; if `{name}` also names an array, rename the loop label"
                        ),
                    ),
                };
                let d = Diagnostic::coded(code, span, message)
                    .with_secondary(declared, "loop label declared here");
                match form {
                    ClashForm::Name {
                        kind,
                        declared: Some(at),
                    } => d.with_secondary(at, format!("the {} is declared here", kind.word())),
                    _ => d,
                }
                .with_help(help)
            }
            DiagnosticKind::LabelReused { name, at, outer } => Diagnostic::coded(
                code,
                at,
                format!("loop label `{name}` is already used by an enclosing loop"),
            )
            .with_secondary(outer, "the enclosing loop's label")
            .with_help("rename one of the loop labels"),
            DiagnosticKind::MissingLabel {
                name,
                keyword,
                at,
                suggestion,
                wrap,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("no loop label `{name}` around this `{keyword}`"),
                );
                match suggestion {
                    Some(name) => similar_label(
                        d,
                        at,
                        Some(Suggestion {
                            kind: Definition::LoopLabel,
                            name,
                        }),
                        wrap,
                    ),
                    None => d.with_help("`continue` takes no value"),
                }
            }
            DiagnosticKind::LabelNotOnLoop { at, next } => {
                Diagnostic::coded(code, at, "only a loop can take a loop label")
                    .with_secondary(next, "this is not `loop`, `while` or `for`")
                    .with_help("write the loop label before a loop, or delete it")
            }
            DiagnosticKind::UnnamedLabel { at, remove } => {
                Diagnostic::coded(code, at, "a loop label needs a name")
                    .with_fix(safe_fix("delete `_: `", remove, ""))
            }
            DiagnosticKind::UnusedLabel { name, at, remove } => {
                Diagnostic::coded(code, at, format!("loop label `{name}` is never used"))
                    .with_help("delete the loop label, or name it in a `break` or `continue`")
                    .with_fix(safe_fix("delete the loop label", remove, ""))
            }
            DiagnosticKind::WhileTrue { at, fix } => {
                let d = Diagnostic::coded(code, at, "this `while` has the condition `true`")
                    .with_label("the condition never ends the loop")
                    .with_help("write an endless loop as `loop`");
                if fix {
                    d.with_fix(safe_fix("write `loop`", at, "loop"))
                } else {
                    d
                }
            }
            DiagnosticKind::UnconditionalRecursion {
                kind,
                name,
                at,
                calls,
            } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("{} `{name}` calls itself on every path", kind.word()),
                )
                .with_label("cannot return without calling itself")
                .with_help(format!(
                    "give `{name}` a path that returns without calling itself, such as a base case; to repeat forever, write a `loop` in its body"
                ));
                calls
                    .into_iter()
                    .fold(d, |d, call| d.with_secondary(call, "calls itself here"))
            }
            DiagnosticKind::UnchangedLoopCondition { at, names, lets } => {
                // The other names can never be written, so only a `let` is worth changing.
                let leave = if lets.is_empty() {
                    format!(
                        "{} can't change, so leave the loop with `break`",
                        listed(&names, "and")
                    )
                } else {
                    format!("change {} in the loop or leave it with `break`", listed(&lets, "or"))
                };
                Diagnostic::coded(code, at, "the condition of this `while` never changes")
                    .with_label(format!(
                        "nothing in the loop changes {}",
                        listed(&names, "or")
                    ))
                    .with_help(format!(
                        "{leave}; write `if` to run the body once, or `loop` to run it forever"
                    ))
            }
            DiagnosticKind::UnusedItem {
                kind,
                name,
                at,
                named,
                uses,
            } => unused_name(code, kind, &name, at, None, named, uses),
            DiagnosticKind::UnusedMember {
                kind,
                name,
                at,
                shown,
                uses,
            } => unused_name(code, kind, &name, at, shown, None, uses),
            DiagnosticKind::UnusedBlock { name, at, uses } => {
                let d = Diagnostic::coded(
                    code,
                    at,
                    format!("the `extend` block of `{name}` is never used"),
                )
                .with_label("never used")
                .with_help("delete it");
                cite_uses(d, uses)
            }
            DiagnosticKind::UnusedVariable {
                param,
                name,
                at,
                fix,
            } => {
                let kind = if param { "parameter" } else { "variable" };
                let d = Diagnostic::coded(code, at, format!("{kind} `{name}` is never read"))
                    .with_label("never read");
                if fix.is_empty() {
                    d.with_help(format!("if this is on purpose, name it `_{name}`"))
                } else {
                    let edits = fix.into_iter().map(|span| Edit {
                        span,
                        replacement: "_".to_string(),
                    });
                    d.with_fix(Fix {
                        message: format!("replace `{name}` with `_`"),
                        safe: true,
                        edits: edits.collect(),
                    })
                }
            }
            DiagnosticKind::TypeArguments { name, at, form } => match form {
                ArgumentsForm::Count { want, found } => {
                    let s = if want == 1 { "" } else { "s" };
                    let were = if found == 1 { "was" } else { "were" };
                    Diagnostic::coded(
                        code,
                        at,
                        format!("`{name}` takes {want} type argument{s}, but {found} {were} given"),
                    )
                }
                ArgumentsForm::Missing { params } => {
                    let s = if params.len() == 1 { "" } else { "s" };
                    let each = params
                        .iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Diagnostic::coded(code, at, format!("`{name}` needs its type argument{s}"))
                        .with_help(format!("write `{name}<...>` with a type for {each}"))
                }
                ArgumentsForm::NotGeneric { safe } => {
                    Diagnostic::coded(code, at, format!("`{name}` takes no type arguments"))
                        .with_fix(Fix {
                            message: "remove the type arguments".to_string(),
                            safe,
                            edits: vec![Edit {
                                span: at,
                                replacement: String::new(),
                            }],
                        })
                }
                ArgumentsForm::Value { compare } => {
                    let d = Diagnostic::coded(code, at, format!("`{name}` is not generic"));
                    match compare {
                        Some(compare) => d.with_help(format!("to compare, write `({compare})`")),
                        None => d,
                    }
                }
                ArgumentsForm::Variant { instead } => {
                    let d = Diagnostic::coded(code, at, format!("`{name}` takes no type arguments"));
                    match instead {
                        Some(instead) => d.with_help(format!("write `{instead}`")),
                        None => d,
                    }
                }
            },
            DiagnosticKind::TypeArgument { ty, at } => {
                Diagnostic::coded(code, at, format!("`{ty}` cannot be a type argument"))
            }
            DiagnosticKind::ParamLacks {
                ty,
                op,
                at,
                add,
            } => {
                let d = Diagnostic::coded(code, at, format!("`{ty}` has no `{op}`"))
                    .with_label(format!("this has type `{ty}`"));
                match add {
                    Some(add) => d.with_help(add.help()),
                    None => d,
                }
            }
            DiagnosticKind::InstantiationCycle {
                name,
                at,
                instance,
                through,
                reach,
                choice,
                grows,
            } => {
                let d = through.into_iter().fold(
                    Diagnostic::coded(
                        code,
                        at,
                        format!("`{name}` instantiates itself without end"),
                    )
                    .with_label(instance.label()),
                    |d, (site, hop)| {
                        let text = match hop {
                            Hop::Instantiates(instance) => {
                                format!("which instantiates `{instance}`")
                            }
                            Hop::Calls {
                                by,
                                method,
                                interface,
                            } => match by {
                                None => {
                                    format!("which may call `{method}` through `{interface}`")
                                }
                                Some(by) => format!(
                                    "{} here may call `{method}` through `{interface}`",
                                    by.subject()
                                ),
                            },
                        };
                        d.with_secondary(site, text)
                    },
                );
                let reach = reach.map(|(i, by)| {
                    let lead = by.call_through(&i).unwrap_or_default();
                    format!("{lead}a call through `{i}` counts as reaching every conformance to `{i}`")
                });
                let choice = choice.map(|ty| {
                    format!(
                        "the check is cautious: `{ty}` counts as each choice that a conformance makes, unless the check can show that it is impossible"
                    )
                });
                let grows = grows.map(|(instance, held_by)| match held_by {
                    Some(operand) => format!("`{operand}` holds `{instance}`, whose argument grows"),
                    None => format!("The argument of `{instance}` grows"),
                });
                let help: Vec<String> = reach.into_iter().chain(choice).collect();
                if help.is_empty() {
                    d
                } else {
                    let help = help.join("; ");
                    d.with_help(match grows {
                        Some(grows) => format!("{help}. {grows}"),
                        None => help,
                    })
                }
            }
            DiagnosticKind::ConformanceTooDeep {
                interface,
                root,
                limit,
                holder,
                at,
            } => Diagnostic::coded(
                code,
                at,
                format!("the check of `{interface}` for `{root}` stopped after {limit} nested types"),
            )
            .with_label(format!("`{holder}` is {limit} types deep here"))
            .with_help("a type that nests this deep may instantiate itself without end"),
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
    d.with_fix(unsafe_fix(&similar_message(kind, &name), at, &name))
}

/// The message of the fix that `similar` and `similar_label` give.
fn similar_message(kind: Definition, name: &str) -> String {
    format!(
        "{} {} with a similar name exists: `{name}`",
        kind.article(),
        kind.word()
    )
}

/// `similar`, except that with `wrap`, the start of the `break` that the name at `at` labels, the
/// fix also writes `(` there and `)` after the name: the `break` ends a head, so without the
/// parentheses it would take the `{` after it as its value.
fn similar_label(
    d: Diagnostic,
    at: Span,
    suggestion: Option<Suggestion>,
    wrap: Option<usize>,
) -> Diagnostic {
    let (Some(Suggestion { kind, name }), Some(start)) = (&suggestion, wrap) else {
        return similar(d, at, suggestion);
    };
    let mut fix = unsafe_fix(&similar_message(*kind, name), at, &format!("{name})"));
    fix.edits.insert(
        0,
        Edit {
            span: Span::new(start, start),
            replacement: "(".to_string(),
        },
    );
    d.with_fix(fix)
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

/// The safe fix `message` that replaces `span` with `replacement`.
fn safe_fix(message: &str, span: Span, replacement: &str) -> Fix {
    Fix {
        safe: true,
        ..unsafe_fix(message, span, replacement)
    }
}

/// The last byte of a spec that ends in `?`, which is one ASCII byte.
fn last_byte(spec: Span) -> Span {
    Span::new(spec.end() - 1, spec.end())
}

/// The E0160 label for `this`, which names the kinds it holds inline: `enums` and `structs` are
/// their words. A type that holds neither gets no label.
fn held_label(
    this: &str,
    holds: payload_limit::Holds,
    enums: &str,
    structs: &str,
) -> Option<String> {
    let held = match (holds.enums, holds.structs) {
        (true, true) => format!("{enums} and {structs}"),
        (true, false) => enums.to_string(),
        (false, true) => structs.to_string(),
        (false, false) => return None,
    };
    Some(format!("{this} holds {held} inline"))
}

/// The edits of a removal that keeps the comments in it: `parts` are from `removal_parts`. The
/// first part takes `replacement`, and each other part takes nothing.
fn part_edits(parts: &[Span], replacement: &str) -> Vec<Edit> {
    parts
        .iter()
        .enumerate()
        .map(|(i, &span)| Edit {
            span,
            replacement: if i == 0 { replacement } else { "" }.to_string(),
        })
        .collect()
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

/// `d`, an E0001 on `name`, with the labels and fixes of its `cause`. With `element`, the write is
/// to an element or a `push`, and `d` carries its own label.
fn immutable_cause(d: Diagnostic, name: &str, element: bool, cause: &Immutable) -> Diagnostic {
    match *cause {
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
        Immutable::StructSelf => d.with_help(STRUCT_SELF_HELP),
    }
}

/// The help of W0016 for a copy. The root `root` cannot change for the reason `cause` gives, or
/// can change when there is none. `place` is the place that the copy holds, when it is more than
/// the root: then the help says that the place cannot change. A `hidden` root has the name of the
/// copy, so the copy hides it. Then a root that is a `let` or can change gets no advice that
/// names it. A field and a constant give none: a class step is not a copy, and a constant of a
/// struct or an array is E0085.
pub(crate) fn copy_help(
    cause: Option<&Immutable>,
    root: &str,
    place: Option<&str>,
    hidden: bool,
) -> Option<String> {
    if hidden && matches!(cause, None | Some(Immutable::Let { .. })) {
        return Some("delete the write".to_string());
    }
    let so =
        |here: &str| place.map_or(String::new(), |p| format!(", so `{p}` cannot change{here}"));
    Some(match cause {
        None => "write to the place itself, not to the copy".to_string(),
        Some(Immutable::Let { .. }) => format!(
            "`{root}` is a `let`{}: delete the write, or make `{root}` a `let mut` and write to the place",
            so("")
        ),
        Some(Immutable::Param { .. }) => format!(
            "`{root}` is a plain parameter{}: delete the write, or return the changed copy",
            so(" here")
        ),
        Some(Immutable::StructSelf) => format!(
            "`self` cannot change in this method{}: delete the write, or return the changed copy",
            so(" here")
        ),
        Some(Immutable::Unwrapped { keyword, .. }) => {
            format!(
                "`{root}` is bound by `{keyword}`{}: delete the write",
                so(" here")
            )
        }
        Some(Immutable::Field { .. } | Immutable::Constant { .. }) => return None,
    })
}

/// The help of a write through the plain `self` of a struct method.
const STRUCT_SELF_HELP: &str = "copy `self` into a `let mut` variable";

/// The indefinite article before the type `ty`: "an" before a vowel letter, except the types
/// `u8` and the like, which say "you".
fn indefinite(ty: &str) -> &'static str {
    let unsigned = ty
        .strip_prefix('u')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if !unsigned && ty.starts_with(['a', 'e', 'i', 'o', 'u', 'A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    }
}

/// `d`, an E0169 about the root or field `name`, with the label, the secondary label and the help
/// of its `cause`.
fn mut_not_place(d: Diagnostic, name: &str, cause: &NotPlace) -> Diagnostic {
    let copy = "copy it into a `let mut` local";
    let label = match *cause {
        NotPlace::Range(slice) => format!(
            "a range index gives a new {}, not a place",
            match slice {
                SliceOf::String => "string",
                SliceOf::Array => "array",
            }
        ),
        _ => "not a place".to_string(),
    };
    let d = d.with_label(label);
    match *cause {
        NotPlace::Range(_) | NotPlace::Temporary => d,
        NotPlace::SelfValue { class: true } => {
            d.with_help("an object is shared: `self` can already write every `mut` field")
        }
        NotPlace::SelfValue { class: false } => d.with_help(copy),
        NotPlace::Immutable(Immutable::Let {
            declared,
            redeclared,
        }) => d
            .with_secondary(
                declared,
                if redeclared {
                    format!("`{name}` is redeclared here with `let`")
                } else {
                    "declared with `let` here".to_string()
                },
            )
            .with_help(format!("declare `{name}` with `let mut`")),
        NotPlace::Immutable(Immutable::Param { declared }) => d
            .with_secondary(declared, "declared as a parameter here")
            .with_help(copy),
        NotPlace::Immutable(Immutable::Unwrapped { keyword, declared }) => d
            .with_secondary(declared, format!("bound by `{keyword}` here"))
            .with_help(copy),
        NotPlace::Immutable(Immutable::Field { declared }) => d
            .with_secondary(declared, "declared without `mut` here")
            .with_help(format!("declare the field `{name}` as `mut`")),
        NotPlace::Immutable(Immutable::Constant { declared }) => {
            d.with_secondary(declared, "declared with `const` here")
        }
        NotPlace::Immutable(Immutable::StructSelf) => d.with_help(STRUCT_SELF_HELP),
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

    /// One, two and three types read as "an `A`", "an `A` or an `B`" and "an `A`, an `B` or an
    /// `C`".
    #[test]
    fn one_of_should_join_types_with_or() {
        let types = |n: usize| -> Vec<String> {
            ["[i64]", "[f64]", "[u8]"][..n]
                .iter()
                .map(|t| (*t).to_owned())
                .collect()
        };
        assert_eq!(one_of(&types(1)), "an `[i64]`");
        assert_eq!(one_of(&types(2)), "an `[i64]` or an `[f64]`");
        assert_eq!(one_of(&types(3)), "an `[i64]`, an `[f64]` or an `[u8]`");
    }

    /// A cycle of up to `CYCLE_SHOWN + 1` entries shows all of them: "and 1 more" is no shorter
    /// than the entry. A longer one shows `CYCLE_SHOWN` and counts the rest.
    #[test]
    fn cycle_listed_should_cut_only_a_list_two_longer_than_the_limit() {
        let names = |n: usize| -> Vec<String> { (0..n).map(|k| format!("A{k}")).collect() };
        assert_eq!(
            cycle_listed(&names(CYCLE_SHOWN + 1)),
            "`A0`, `A1`, `A2`, `A3`, `A4` and `A5`"
        );
        assert_eq!(
            cycle_listed(&names(CYCLE_SHOWN + 2)),
            "`A0`, `A1`, `A2`, `A3`, `A4` and 2 more"
        );
        assert_eq!(cycle_shown(&names(CYCLE_SHOWN + 1)).1, 0);
        assert_eq!(
            cycle_shown(&names(CYCLE_SHOWN + 2)),
            (&names(CYCLE_SHOWN)[..], 2)
        );
    }

    /// A language rule has no place to point at, so a conformance error shows it as its label.
    #[test]
    fn a_conformance_error_should_label_a_rule_that_has_no_span() {
        let d: Diagnostic = DiagnosticKind::NotConform {
            ty: "i64".to_owned(),
            interface: "Shape".to_owned(),
            at: Span::new(0, 1),
            form: NotConformForm::Any {
                origin: Some(MismatchOrigin::Condition),
            },
        }
        .into();
        assert!(d.label().is_some());
        assert_eq!(d.secondary(), []);
    }

    #[test]
    fn instances_should_write_the_singular_for_one() {
        assert_eq!(instances(0), "0 instances");
        assert_eq!(instances(1), "1 instance");
        assert_eq!(instances(34_466), "34,466 instances");
    }

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
