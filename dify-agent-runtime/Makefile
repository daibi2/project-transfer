.PHONY: build clean test lint gen-cli-help integration integration-up integration-test integration-down sync-e2b-template-dev sync-e2b-template-prod sync-e2b-template-dry-run
BIN_DIR := bin
AGENT_CLI_HELP_JSON := ../dify-agent/src/dify_agent/layers/_agent_cli_help.json
CARGO ?= cargo
CARGO_TARGET_DIR ?= target
RELEASE_DIR := $(CARGO_TARGET_DIR)/release

# SQLite and zstd are bundled C libraries (compiled at build time). Images no
# longer use CGO_ENABLED=0; the runtime stage still ships ordinary ELF
# binaries linked against glibc, with sqlite/zstd statically compiled in.

build: $(BIN_DIR)/shellctl $(BIN_DIR)/shellctl-sanitize-pty $(BIN_DIR)/shellctl-runner-exit $(BIN_DIR)/shellctl-runner $(BIN_DIR)/dify-agent gen-cli-help

$(RELEASE_DIR)/shellctl $(RELEASE_DIR)/shellctl-sanitize-pty $(RELEASE_DIR)/shellctl-runner-exit $(RELEASE_DIR)/shellctl-runner $(RELEASE_DIR)/dify-agent: $(shell find src Cargo.toml Cargo.lock -type f 2>/dev/null)
	$(CARGO) build --release

$(BIN_DIR)/shellctl: $(RELEASE_DIR)/shellctl
	mkdir -p $(BIN_DIR)
	cp $< $@

$(BIN_DIR)/shellctl-sanitize-pty: $(RELEASE_DIR)/shellctl-sanitize-pty
	mkdir -p $(BIN_DIR)
	cp $< $@

$(BIN_DIR)/shellctl-runner-exit: $(RELEASE_DIR)/shellctl-runner-exit
	mkdir -p $(BIN_DIR)
	cp $< $@

$(BIN_DIR)/shellctl-runner: $(RELEASE_DIR)/shellctl-runner
	mkdir -p $(BIN_DIR)
	cp $< $@

$(BIN_DIR)/dify-agent: $(RELEASE_DIR)/dify-agent
	mkdir -p $(BIN_DIR)
	cp $< $@

gen-cli-help: $(BIN_DIR)/dify-agent
	$(BIN_DIR)/dify-agent __dump-cli-help > $(AGENT_CLI_HELP_JSON)

test: lint
	$(CARGO) test

lint:
	$(CARGO) clippy --all-targets

clean:
	rm -rf $(BIN_DIR)
	$(CARGO) clean

IMAGE_NAME := dify-agent-runtime:test
STATE_FILE := .integration-state

integration-up:
	@echo "Building runtime image..."
	docker build -t $(IMAGE_NAME) -f docker/Dockerfile .
	$(eval TEST_ID := $(shell printf '%05d' $$((RANDOM % 100000))))
	$(eval CONTAINER_NAME := sandbox-rt-$(TEST_ID))
	$(eval HOST_PORT := $(shell python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()'))
	$(eval AUTH_TOKEN := test-token-$(TEST_ID))
	@echo "Starting runtime container $(CONTAINER_NAME) on port $(HOST_PORT)..."
	docker run -d --name $(CONTAINER_NAME) \
		-p $(HOST_PORT):5004 \
		-e SHELLCTL_AUTH_TOKEN=$(AUTH_TOKEN) \
		$(IMAGE_NAME)
	@echo 'CONTAINER_NAME=$(CONTAINER_NAME)' > $(STATE_FILE)
	@echo 'HOST_PORT=$(HOST_PORT)' >> $(STATE_FILE)
	@echo 'AUTH_TOKEN=$(AUTH_TOKEN)' >> $(STATE_FILE)
	@echo "Waiting for runtime to be ready..."
	@for i in $$(seq 1 30); do \
		if curl -sf http://localhost:$(HOST_PORT)/healthz > /dev/null 2>&1; then \
			echo "Runtime is ready on port $(HOST_PORT)"; \
			break; \
		fi; \
		if [ "$$i" -eq 30 ]; then \
			echo "ERROR: runtime not ready after 60s" >&2; \
			docker logs $(CONTAINER_NAME); \
			docker rm -f $(CONTAINER_NAME) 2>/dev/null; \
			rm -f $(STATE_FILE); \
			exit 1; \
		fi; \
		sleep 2; \
	done
	$(eval CONTAINER_NAME_NOISO := sandbox-rt-noiso-$(TEST_ID))
	$(eval HOST_PORT_NOISO := $(shell python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()'))
	$(eval AUTH_TOKEN_NOISO := test-token-noiso-$(TEST_ID))
	@echo "Starting no-isolation container $(CONTAINER_NAME_NOISO) on port $(HOST_PORT_NOISO)..."
	docker run -d --name $(CONTAINER_NAME_NOISO) \
		-p $(HOST_PORT_NOISO):5004 \
		-e SHELLCTL_AUTH_TOKEN=$(AUTH_TOKEN_NOISO) \
		-e SHELLCTL_ENABLE_PATH_ISOLATION=false \
		$(IMAGE_NAME)
	@echo 'CONTAINER_NAME_NOISO=$(CONTAINER_NAME_NOISO)' >> $(STATE_FILE)
	@echo 'HOST_PORT_NOISO=$(HOST_PORT_NOISO)' >> $(STATE_FILE)
	@echo 'AUTH_TOKEN_NOISO=$(AUTH_TOKEN_NOISO)' >> $(STATE_FILE)
	@for i in $$(seq 1 30); do \
		if curl -sf http://localhost:$(HOST_PORT_NOISO)/healthz > /dev/null 2>&1; then \
			echo "No-isolation runtime is ready on port $(HOST_PORT_NOISO)"; \
			break; \
		fi; \
		if [ "$$i" -eq 30 ]; then \
			echo "ERROR: no-isolation runtime not ready after 60s" >&2; \
			docker logs $(CONTAINER_NAME_NOISO); \
			docker rm -f $(CONTAINER_NAME_NOISO) 2>/dev/null; \
			exit 1; \
		fi; \
		sleep 2; \
	done

integration-test:
	@test -f $(STATE_FILE) || { echo "ERROR: run 'make integration-up' first" >&2; exit 1; }
	@. ./$(STATE_FILE); \
	SHELLCTL_GO_URL=http://localhost:$$HOST_PORT \
	SHELLCTL_TEST_TOKEN=$$AUTH_TOKEN \
	SHELLCTL_GO_URL_NO_ISOLATION=http://localhost:$$HOST_PORT_NOISO \
	SHELLCTL_TEST_TOKEN_NO_ISOLATION=$$AUTH_TOKEN_NOISO \
	$(CARGO) test --test acceptance -- --ignored --nocapture --test-threads=1

integration-logs:
	@test -f $(STATE_FILE) || { echo "ERROR: no state file" >&2; exit 1; }
	@. ./$(STATE_FILE); docker logs $$CONTAINER_NAME

integration-down:
	@if [ -f $(STATE_FILE) ]; then \
		. ./$(STATE_FILE); \
		docker rm -f $$CONTAINER_NAME 2>/dev/null || true; \
		docker rm -f $$CONTAINER_NAME_NOISO 2>/dev/null || true; \
		rm -f $(STATE_FILE); \
	fi

integration:
	@$(MAKE) integration-up
	@trap '$(MAKE) integration-down' EXIT; \
	$(MAKE) integration-test

sync-e2b-template-dry-run:
	./docker/sync-e2b-template.sh --dry-run

sync-e2b-template-dev:
	E2B_API_KEY=$(E2B_API_KEY_DEV) ./docker/sync-e2b-template.sh

sync-e2b-template-prod:
	E2B_API_KEY=$(E2B_API_KEY_PROD) ./docker/sync-e2b-template.sh

benchmark:
	$(CARGO) test --release --test acceptance -- --ignored --nocapture --test-threads=1 || true
