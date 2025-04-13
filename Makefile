.PHONY: install-coverage coverage

install-coverage:
	cargo +stable install cargo-llvm-cov --locked

coverage:
	cargo llvm-cov --html
