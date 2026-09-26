.DEFAULT_GOAL := help
.PHONY: help build bundle run restart test

help:
	@printf '%s\n' \
	  'make build    Build the release executable' \
	  'make bundle   Build the macOS app' \
	  'make run      Build and open the macOS app' \
	  'make restart  Rebuild and relaunch (closes current sessions)' \
	  'make test     Run the test suite'

build:
	cargo build --release

bundle:
	@sh scripts/bundle.sh

run: bundle
	open -n "target/bundle/Arabic Term.app"

restart: bundle
	@sh scripts/restart.sh

test:
	cargo test
