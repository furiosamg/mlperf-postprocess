SHELL := /bin/bash -o pipefail

.PHONY: lint test

check-docker-tag:
ifndef DOCKER_TAG
	$(error "DOCKER_TAG is not set")
endif

lint:
	cargo fmt --all --check \
	&& cargo -q clippy --release --all-targets -- -D rust_2018_idioms -D warnings

test:
	cargo test --release

docker-build: check-docker-tag
	DOCKER_BUILDKIT=1 docker build -t asia-northeast3-docker.pkg.dev/next-gen-infra/furiosa-ai/mlperf-postprocess:${DOCKER_TAG} --secret id=furiosa.conf,src=/etc/apt/auth.conf.d/furiosa.conf -f docker/Dockerfile ./docker/

docker-push: check-docker-tag
	docker push asia-northeast3-docker.pkg.dev/next-gen-infra/furiosa-ai/mlperf-postprocess:${DOCKER_TAG}

docker-wheel:
	DOCKER_BUILDKIT=1 docker build -t mlperf-postprocess-wheel -f docker/wheel.Dockerfile docker

wheel: docker-wheel
	docker run --rm -it \
		-v `pwd`/wheels:/app/target/wheels \
		-v `pwd`:/app \
		mlperf-postprocess-wheel \
		maturin build --release --manylinux 2014 -i python3.8 python3.9 python3.10
