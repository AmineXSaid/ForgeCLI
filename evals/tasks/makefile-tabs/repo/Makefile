.PHONY: lint test build

lint:
	python3 -m py_compile app.py

test:
	python3 -m unittest -q

build: test
	mkdir -p dist && cp app.py dist/
