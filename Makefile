# Override the data location with: make sqlite LEMMA_DATA=/path/to/lemma-data
LEMMA_DATA ?= ../lemma-data

export KAIKKI_LOCAL_DIR ?= $(abspath $(LEMMA_DATA)/kaikki)
export DILEMMA_DATA_DIR ?= $(abspath $(LEMMA_DATA)/dilemma)

# Without these the build still succeeds, but with fewer inflected forms and no
# English translations, so a missing file stops it instead.
REQUIRED_DATA := $(KAIKKI_LOCAL_DIR)/en/kaikki.org-dictionary-English.jsonl.gz \
                 $(DILEMMA_DATA_DIR)/mg_lookup_scored.json \
                 $(DILEMMA_DATA_DIR)/mg_ranked_forms.json

.DEFAULT_GOAL := help
.PHONY: help sqlite

help: ## List the available commands
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  %-10s %s\n", $$1, $$2}'

sqlite: ## Build lemma_greek_en/lemma_greek_en.db with the full lemma-data
	@for f in $(REQUIRED_DATA); do \
		[ -f "$$f" ] || { echo "Missing data file: $$f" >&2; exit 1; }; \
	done
	cargo run --release --bin lemma -- --sqlite
