.PHONY: build release test lint fmt clean hooks

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean

hooks:
	cp hooks/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	cp hooks/pre-push .git/hooks/pre-push
	chmod +x .git/hooks/pre-push
