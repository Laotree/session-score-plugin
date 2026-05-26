.PHONY: build release test lint fmt clean hooks install

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

install: release
	mkdir -p ~/.local/bin
	cp target/release/session-score-plugin ~/.local/bin/session-score-plugin
	~/.local/bin/session-score-plugin install
