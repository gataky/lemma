# Override the data location with: make data sqlite LEMMA_DATA=/path/to/lemma-data
LEMMA_DATA ?= ../lemma-data

export KAIKKI_LOCAL_DIR ?= $(abspath $(LEMMA_DATA)/kaikki)
export DILEMMA_DATA_DIR ?= $(abspath $(LEMMA_DATA)/dilemma)

KAIKKI_URL  := https://kaikki.org/dictionary
DILEMMA_URL := https://huggingface.co/datasets/open-greek/dilemma-data/resolve/main

GREEK_DUMP   := $(KAIKKI_LOCAL_DIR)/en-el/kaikki.org-dictionary-Greek-words.jsonl
ENGLISH_DUMP := $(KAIKKI_LOCAL_DIR)/en/kaikki.org-dictionary-English.jsonl.gz
DILEMMA_DATA := $(DILEMMA_DATA_DIR)/mg_lookup_scored.json \
                $(DILEMMA_DATA_DIR)/mg_ranked_forms.json

# Without these the build still succeeds, but downloads the Greek dump into the
# repo and gives fewer inflected forms and no English translations, so a
# missing file stops it instead.
DATA := $(GREEK_DUMP) $(ENGLISH_DUMP) $(DILEMMA_DATA)

# Download to a .part file and rename it when complete, so an interrupted
# download is never taken for a finished one. -R keeps the server's
# Last-Modified time: lemma reads the Wiktionary extraction date from it.
fetch = curl -L --fail -R --create-dirs --progress-bar -o "$@.part" "$(1)" && mv "$@.part" "$@"

.DEFAULT_GOAL := help
.PHONY: help data sqlite

help: ## List the available commands
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  %-10s %s\n", $$1, $$2}'

data: $(DATA) ## Download the data sqlite needs into LEMMA_DATA (missing files only, about 1 GB)

sqlite: ## Build lemma_greek_en/lemma_greek_en.db with the full lemma-data
	@for f in $(DATA); do \
		[ -f "$$f" ] || { echo "Missing data file: $$f (run make data)" >&2; exit 1; }; \
	done
	cargo run --release --bin lemma

# kaikki.org names the Greek dump differently from the local copy lemma reads.
$(GREEK_DUMP):
	$(call fetch,$(KAIKKI_URL)/Greek/kaikki.org-dictionary-Greek.jsonl)

$(ENGLISH_DUMP):
	$(call fetch,$(KAIKKI_URL)/English/kaikki.org-dictionary-English.jsonl.gz)

$(DILEMMA_DATA_DIR)/%.json:
	$(call fetch,$(DILEMMA_URL)/$*.json)
