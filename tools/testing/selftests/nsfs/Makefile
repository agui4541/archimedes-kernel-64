TEST_PROGS := owner pidns

CFLAGS := -Wall -Wno-error

all: owner pidns
owner: owner.c
pidns: pidns.c

clean:
	$(RM) owner pidns

include ../lib.mk
