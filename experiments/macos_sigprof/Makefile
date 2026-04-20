CC ?= cc
CFLAGS_COMMON = -O2 -g -Wall -Wextra -pthread

# MODE values in the C source: RACE=1, STEADY=2
# STACK_SIZE: 2 MiB = 2097152, 16 MiB = 16777216

all: repro_2m_race repro_2m_steady repro_16m_race

repro_2m_race: repro.c
	$(CC) $(CFLAGS_COMMON) -DSTACK_SIZE=$$((2*1024*1024)) -DMODE=1 -DITERATIONS=8000 repro.c -o $@

repro_2m_steady: repro.c
	$(CC) $(CFLAGS_COMMON) -DSTACK_SIZE=$$((2*1024*1024)) -DMODE=2 -DITERATIONS=8000 repro.c -o $@

repro_16m_race: repro.c
	$(CC) $(CFLAGS_COMMON) -DSTACK_SIZE=$$((16*1024*1024)) -DMODE=1 -DITERATIONS=8000 repro.c -o $@

clean:
	rm -f repro_2m_race repro_2m_steady repro_16m_race

.PHONY: all clean
