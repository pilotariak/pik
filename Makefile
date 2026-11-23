# SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
# SPDX-License-Identifier: Apache-2.0

BANNER = P I K

SHELL = /bin/bash -o pipefail

DIR = $(shell pwd)

# Colors for terminal output
NO_COLOR=\033[0m
OK_COLOR=\033[32;01m
ERROR_COLOR=\033[31;01m
WARN_COLOR=\033[33;01m
INFO_COLOR=\033[36m
WHITE_COLOR=\033[1m
MAKE_COLOR=\033[33;01m%-20s\033[0m

.DEFAULT_GOAL := help

# Define common messages
# OK=[✅]
# KO=[🔴]
# WARN=[⚠️]
# INFO=[🔵]
OK=[🟢]
KO=[🔴]
WARN=[🟠]
INFO=[🔵]


.PHONY: help
help:
	@echo -e "$(OK_COLOR)      $(BANNER)$(NO_COLOR)"
	@echo "------------------------------------------------------------------"
	@echo ""
	@awk 'BEGIN {FS = ":.*##"; printf "Usage: make ${INFO_COLOR}<target>${NO_COLOR}\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  ${INFO_COLOR}%-25s${NO_COLOR} %s\n", $$1, $$2 } /^##@/ { printf "\n${WHITE_COLOR}%s${NO_COLOR}\n", substr($$0, 5) } ' $(MAKEFILE_LIST)
	@echo ""

guard-%:
	@if [ "${${*}}" = "" ]; then \
		echo -e "$(ERROR_COLOR)Environment variable $* not set$(NO_COLOR)"; \
		exit 1; \
	fi

check-%:
	@if $$(hash $* 2> /dev/null); then \
		echo -e "$(OK_COLOR)$(OK)$(NO_COLOR) $*"; \
	else \
		echo -e "$(ERROR_COLOR)$(KO)$(NO_COLOR) $*"; \
	fi

WORKER_BUILD_VERSION = 0.9.0

##@ Development

.PHONY: install
install: ## Install the Rust toolchain, worker-build and the npm dependencies
	@echo -e "$(INFO)$(INFO_COLOR)[Install] Installing dependencies$(NO_COLOR)"
	rustup toolchain install
	cargo install --locked worker-build@$(WORKER_BUILD_VERSION)
	npm ci

.PHONY: build
build: ## Build the Worker into build/ (worker-build)
	@echo -e "$(INFO)$(INFO_COLOR)[Build] Building the Worker$(NO_COLOR)"
	worker-build --release

.PHONY: dev
dev: ## Start local dev server (wrangler dev)
	@echo -e "$(INFO)$(INFO_COLOR)[Dev] Starting Cloudflare Worker$(NO_COLOR)"
	npx wrangler dev

.PHONY: db-migrate-local
db-migrate-local: ## Apply the D1 migrations to the local database
	@echo -e "$(INFO)$(INFO_COLOR)[DB] Applying migrations locally$(NO_COLOR)"
	npx wrangler d1 migrations apply DB --local

.PHONY: check
check: ## Check formatting and run clippy
	@echo -e "$(INFO)$(INFO_COLOR)[Check] cargo fmt and clippy$(NO_COLOR)"
	cargo fmt --check
	cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings

.PHONY: test
test: build ## Run the Rust unit tests, then the Worker tests in Miniflare
	@echo -e "$(INFO)$(INFO_COLOR)[Test] Running tests$(NO_COLOR)"
	cargo test
	npm test

.PHONY: lint
lint: ## Run the prek hooks on all files
	@echo -e "$(INFO)$(INFO_COLOR)[Lint] Running prek hooks $(NO_COLOR)"
	@prek run -a

##@ Deployment

.PHONY: deploy
deploy: ## Apply the D1 migrations remotely, then deploy the Worker
	@echo -e "$(INFO)$(INFO_COLOR)[Deploy] Deploying to production$(NO_COLOR)"
	npx wrangler d1 migrations apply DB --remote
	npx wrangler deploy

##@ Maintenance

.PHONY: clean
clean: ## Clean project
	@echo -e "$(INFO)$(INFO_COLOR)[Clean] Processing $(NO_COLOR)"
	cargo clean
	rm -rf build node_modules .wrangler
