.DEFAULT_GOAL := help

.PHONY: help cli-build cli-install cli-uninstall cli-help

help:                 ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

# --- C L I ----------------
cli-build:            ## Build the flatbed CLI (debug)
	@cargo build -p flatbed_build --bin flatbed

cli-install:          ## Install the flatbed CLI to ~/.cargo/bin (release build, overwrites existing)
	@cargo install --path crates/flatbed_build --bin flatbed --locked --force
	@flatbed --version

cli-uninstall:        ## Remove the flatbed CLI from ~/.cargo/bin
	@cargo uninstall flatbed_build

cli-help:             ## Show the flatbed CLI help
	@cargo run -q -p flatbed_build --bin flatbed -- --help
