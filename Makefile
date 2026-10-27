# The one place to run Sisu's commands. `make help` lists them. Each target
# is one line, a script in scripts/ or a cargo command; the scripts hold the logic.

# Plain = so an exported BASE in the environment never leaks in; make BASE=x still wins.
BASE =
SECS =
BENCH =

.DEFAULT_GOAL := help
.PHONY: help setup audit-setup build test bless lint deny miri check coverage mutants verify fuzz fuzz-coverage bench callgrind site

help: ## List the targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-z-]+:.*## / { printf "  %-12s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo
	@echo 'BASE=<commit> sets where the changes start: by default where HEAD left main.'
	@echo 'On a stacked branch, pass the stage below, so its changes stay out.'

setup: ## Once per clone: LLVM 23, the pre-commit and commit-msg hooks, cargo-shear, cargo-mutants, cargo-llvm-cov, cargo-fuzz
	scripts/install-llvm.sh
	prek install -t pre-commit -t commit-msg
	cargo install --locked cargo-shear
	cargo install --locked cargo-mutants
	cargo install --locked cargo-llvm-cov
	cargo install --locked cargo-fuzz@0.13.2

audit-setup: ## For the audit-sisu skill: add opt and clang, with the ASan runtime, to .llvm/23
	scripts/install-llvm.sh --audit

build: ## Build sisu and the two runtime archives
	cargo build --workspace

test: ## Run every test with nextest, whose setup script builds the runtime
	cargo nextest run --workspace --locked
	list=$$(mktemp) && cargo nextest list --workspace --locked --message-format json > $$list && python3 scripts/nextest_filters.py .config/nextest.toml $$list; status=$$?; rm -f $$list; exit $$status

bless: ## Fill the expected .out/.err/.json files of the end-to-end cases and the site's programs
	SISU_BLESS=1 cargo nextest run -E 'binary(cases)'

lint: ## The hooks CI runs: fmt, clippy, typos, shellcheck and the rest
	prek run --all-files

deny: ## Audit the dependencies: advisories, licenses, sources
	cargo deny check

miri: ## Run the runtime's unit tests under Miri (nightly), which stops at undefined behaviour
	cargo +nightly miri test -p sisu-runtime

check: lint test deny ## Lint, test and deny; CI also runs make miri

coverage: ## Coverage of the changes since BASE
	scripts/coverage.sh "$(BASE)"

mutants: ## Mutation-test the changes since BASE
	scripts/mutants.sh "$(BASE)"

verify: ## Coverage, then mutants, since BASE; the last line is the verdict; LOG=<file> writes it there
	scripts/verify.sh "$(BASE)" $(if $(LOG),"$(LOG)")

fuzz: ## Fuzz TARGET (front_end by default, or program) for SECS seconds (1800 by default)
	scripts/fuzz.sh --target "$(or $(TARGET),front_end)" "$(SECS)"

fuzz-coverage: ## Line coverage per compiler stage of TARGET's corpus (and front_end's seeds); front_end by default
	scripts/fuzz.sh --target "$(or $(TARGET),front_end)" --coverage

bench: ## Time the benchmarks in bench/ (BENCH='fib sieve' picks some)
	scripts/bench.sh $(BENCH)

callgrind: ## Count the instructions of sisu check, as CodSpeed does
	scripts/codspeed-prep.sh
	valgrind --tool=callgrind --callgrind-out-file=/dev/null target/release/sisu check target/codspeed/large/main.sisu

site: ## Build the site into site/_site and check it
	uv run --locked --script site/build.py --test && uv run --locked --script site/build.py
