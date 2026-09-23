#
# Makefile for the linux alpha-specific parts of the memory manager.
#

ccflags-y := -Wno-error

obj-y	:= init.o fault.o

obj-$(CONFIG_DISCONTIGMEM) += numa.o
