MODEL     ?= n4f1v20
STEP      ?= step_fair
INVARIANT ?= safety_inv
MAX_STEPS ?= 1000
SAMPLES   ?= 1
SEED      ?=
TRACE     ?= trace.itf.json
REPORT    ?= analisys.md

MUTANT_SAMPLES   ?= 300
MUTANT_MAX_STEPS ?= 300

.PHONY: test mutants sim analyze

test:
	quint test scenarios_test.qnt --main=scenariosTest

mutants:
	python3 tools/mutants.py --samples $(MUTANT_SAMPLES) --max-steps $(MUTANT_MAX_STEPS)

sim:
	quint run models.qnt --main=$(MODEL) --step=$(STEP) --invariant=$(INVARIANT) \
		--max-steps=$(MAX_STEPS) --max-samples=$(SAMPLES) --out-itf=$(TRACE) \
		$(if $(SEED),--seed=$(SEED))

analyze:
	python3 tools/analyze.py $(TRACE) $(REPORT)
