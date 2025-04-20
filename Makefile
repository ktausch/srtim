.PHONY: install-coverage coverage cloc

install-coverage:
	cargo +stable install cargo-llvm-cov --locked

coverage:
	cargo llvm-cov --html

cloc:
	cloc src --include-lang=rust
