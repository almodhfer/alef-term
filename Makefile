.DEFAULT_GOAL := help
.PHONY: help build bundle install run restart test

ifeq ($(shell uname -s),Darwin)

help:
	@printf '%s\n' \
	  'make build    Build the release executable' \
	  'make bundle   Build the macOS app' \
	  'make run      Build and open the macOS app' \
	  'make restart  Rebuild and relaunch (closes current sessions)' \
	  'make test     Run the test suite'

bundle:
	@sh scripts/bundle.sh

run: bundle
	open -n "target/bundle/Arabic Term.app"

restart: bundle
	@sh scripts/restart.sh

else

help:
	@printf '%s\n' \
	  'make build    Build the release executable' \
	  'make install  Install for this user, with a launcher entry' \
	  'make run      Build and open a window' \
	  'make restart  Rebuild and relaunch (closes current sessions)' \
	  'make test     Run the test suite'

install:
	@sh scripts/install-linux.sh

run: build
	@setsid nohup ./target/release/arabic-term </dev/null >/dev/null 2>&1 &

restart: build
	@sh scripts/restart-linux.sh

endif

build:
	cargo build --release

test:
	cargo test
