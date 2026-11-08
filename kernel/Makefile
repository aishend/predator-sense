obj-m += predator_wmi.o

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build

# Kernels built with clang (e.g. CachyOS) need the LLVM toolchain for modules.
ifneq ($(shell grep -s '^CONFIG_CC_IS_CLANG=y' $(KDIR)/.config),)
  KMAKE_FLAGS := LLVM=1
endif

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(KMAKE_FLAGS) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(KMAKE_FLAGS) clean
