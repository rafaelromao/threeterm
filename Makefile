PREFIX ?= $(HOME)/.local
JOBS ?= 2
.DEFAULT_GOAL := help

.PHONY: install uninstall test-install help

install:
	bash scripts/install-local.sh --prefix "$(PREFIX)" --jobs "$(JOBS)" $(INSTALL_FLAGS)

uninstall:
	bash scripts/install-local.sh --prefix "$(PREFIX)" --uninstall

test-install:
	bash tests/install-contract.sh

help:
	@printf '%s\n' 'make install              Build and install ThreeTerm and its dependencies (Arch Linux).' 'make install JOBS=4       Set build parallelism (default: 2).' 'make install PREFIX=...  Choose an installation prefix (default: ~/.local).' 'make uninstall           Remove ThreeTerm from the selected prefix.' 'make test-install        Check the installer contract without changing system packages.'
