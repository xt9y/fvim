.PHONY: all test install clean

ifeq ($(OS),Windows_NT)
RUN = powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/make.ps1
else
RUN = sh scripts/make.sh
endif

all:
	$(RUN) build

test:
	$(RUN) test

install:
	$(RUN) install

clean:
	$(RUN) clean
