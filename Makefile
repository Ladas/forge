# -------------------------------------------------------------------
# Configuration
# -------------------------------------------------------------------

NIGHTLY          ?= nightly
V                ?=

LINT_CMDS        := cargo cargo-machete
LINT_EXTRA_CMDS  := typos taplo shellcheck actionlint
AUDIT_CMDS       := cargo-audit cargo-deny

ifneq ($(V),)
  _NOCAPTURE := -- --nocapture
endif

.PHONY: all build release check clean \
	test mutants lint lint-extra fmt doc audit semver \
	coverage coverage-check \
	check-prereqs check-prereqs-extra check-prereqs-audit check-prereqs-nightly \
	setup-hooks \
	help

# -------------------------------------------------------------------
# All
# -------------------------------------------------------------------

all: build lint lint-extra test audit

# -------------------------------------------------------------------
# Build
# -------------------------------------------------------------------

build:
	cargo build --all-targets --features test-support

release:
	cargo build --release

check:
	cargo check --all-targets --features test-support

clean:
	cargo clean

# -------------------------------------------------------------------
# Test
# -------------------------------------------------------------------

test:
	cargo test --features test-support $(_NOCAPTURE)

mutants:
	cargo mutants

# -------------------------------------------------------------------
# Prerequisites
# -------------------------------------------------------------------

check-prereqs:
	@for cmd in $(LINT_CMDS); do \
		command -v "$$cmd" >/dev/null 2>&1 || { \
			echo "\"$$cmd\" is not installed" >&2; \
			exit 1; \
		}; \
	done

check-prereqs-extra:
	@for cmd in $(LINT_EXTRA_CMDS); do \
		command -v "$$cmd" >/dev/null 2>&1 || { \
			echo "\"$$cmd\" is not installed" >&2; \
			exit 1; \
		}; \
	done

check-prereqs-audit:
	@for cmd in $(AUDIT_CMDS); do \
		command -v "$$cmd" >/dev/null 2>&1 || { \
			echo "\"$$cmd\" is not installed" >&2; \
			exit 1; \
		}; \
	done

check-prereqs-nightly:
	@cargo +$(NIGHTLY) fmt --version >/dev/null 2>&1 || { \
		echo "nightly rustfmt is not installed - run \"rustup toolchain install $(NIGHTLY) --component rustfmt\"" >&2; \
		exit 1; \
	}

# -------------------------------------------------------------------
# Quality
# -------------------------------------------------------------------

lint: check-prereqs check-prereqs-nightly
	cargo clippy --all-targets --features test-support -- -D warnings
	cargo +$(NIGHTLY) fmt --all -- --check
	cargo machete

lint-extra: check-prereqs-extra
	typos
	taplo fmt --check
	shellcheck .hooks/pre-commit
	actionlint

fmt: check-prereqs-nightly
	cargo +$(NIGHTLY) fmt --all

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

audit: check-prereqs-audit
	cargo audit
	cargo deny check

coverage:
	cargo llvm-cov --html --output-dir target/coverage \
		--ignore-filename-regex 'src/main\.rs' \
		--fail-under-lines 90 \
		--fail-under-regions 80

coverage-check:
	cargo llvm-cov \
		--ignore-filename-regex 'src/main\.rs' \
		--fail-under-lines 90 \
		--fail-under-regions 80

semver:
	cargo semver-checks

# -------------------------------------------------------------------
# Dev Setup
# -------------------------------------------------------------------

setup-hooks:
	@ln -sf ../../.hooks/pre-commit .git/hooks/pre-commit
	@echo "Git hooks installed"

# -------------------------------------------------------------------
# Help
# -------------------------------------------------------------------

help:
	@echo "Variables:"
	@echo "  V=1                show test output (--nocapture)"
	@echo "  NIGHTLY            nightly toolchain name for rustfmt"
	@echo ""
	@echo "Top-level:"
	@echo "  all              build + lint + lint-extra + test + audit"
	@echo ""
	@echo "Build:"
	@echo "  build            cargo build --all-targets"
	@echo "  release          cargo build --release"
	@echo "  check            cargo check --all-targets"
	@echo "  clean            cargo clean"
	@echo ""
	@echo "Test:"
	@echo "  test             run all tests"
	@echo "  mutants          mutation testing (cargo-mutants)"
	@echo ""
	@echo "Quality:"
	@echo "  lint             clippy + rustfmt check + machete"
	@echo "  lint-extra       typos + taplo + shellcheck + actionlint"
	@echo "  fmt              format with nightly rustfmt"
	@echo "  doc              build docs with warnings denied"
	@echo "  audit            cargo audit + cargo deny"
	@echo "  semver           cargo semver-checks"
	@echo "  coverage         HTML coverage report"
	@echo "  coverage-check   fail if lines < 90%% or regions < 80%%"
	@echo ""
	@echo "Dev Setup:"
	@echo "  setup-hooks      install git pre-commit hook"
