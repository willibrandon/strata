# Builds all Strata guest software. C is translated to assembly by clang
# (-S); all assembling and linking is done by strata-as (docs/toolchain.md).
#
#   make            boot ROM, kernel, programs, flash contents
#   make clean
#
# Outputs in build/sw/: boot/boot.{hex,elf,sdb,lst}, kernel/kernel.*,
# user/<prog>/<prog>.*, system.img (boot image), flash.bin (flash contents
# from FLASH_IMAGE_OFFSET: boot image, then the program store).

ROOT   := $(abspath ..)
BUILD  := $(ROOT)/build/sw
LLVM   ?= /opt/homebrew/opt/llvm/bin
CC     := $(LLVM)/clang
AS     := $(ROOT)/tools/target/release/strata-as
PACK   := $(ROOT)/tools/target/release/strata-pack
PYTHON ?= python3

TARGET   := --target=riscv32-unknown-elf -march=rv32i_zicsr_zifencei -mabi=ilp32
CFLAGS   := $(TARGET) -mno-relax -mcmodel=medlow -fno-pic -ffreestanding -fno-builtin -nostdlib \
            -fno-asynchronous-unwind-tables -fno-unwind-tables -Os -gline-tables-only \
            -std=gnu11 -Wall -Wextra -Werror -Iinclude -Ikernel
CPPFLAGS := $(TARGET) -Iinclude

layout = $(shell awk '$$1 == "\#define" && $$2 == "$(1)" { print $$3 }' include/strata/layout.h)

LIB     := lib/string.c lib/fmt.c lib/rt.c
KERNEL  := kernel/start.S kernel/main.c kernel/trap.c kernel/task.c kernel/store.c kernel/dev.c \
           kernel/console.c kernel/input.c $(LIB)
# program:slot
PROGRAMS := shell:0 blocks:1 hello:2 counter:2 crash:2

all: $(BUILD)/boot/boot.hex $(BUILD)/flash.bin

# ---------------------------------------------------------------- compile rules
# .c -> .s (clang), .S -> .s (clang preprocessor, keeping line markers)
$(BUILD)/obj/%.c.s: %.c | tools
	@mkdir -p $(dir $@)
	@echo "  CC   $<"; $(CC) $(CFLAGS) -S $< -o $@

$(BUILD)/obj/%.S.s: %.S
	@mkdir -p $(dir $@)
	$(CC) $(CPPFLAGS) -E -x assembler-with-cpp $< -o $@

objs = $(patsubst %,$(BUILD)/obj/%.s,$(1))

# ---------------------------------------------------------------- boot ROM
$(BUILD)/boot/boot.hex: $(call objs,boot/boot.S) layout/rom.ld | tools
	@mkdir -p $(dir $@)
	$(AS) -T layout/rom.ld -o $(BUILD)/boot/boot.elf --hex $@ --bin $(BUILD)/boot/boot.bin \
	    --debug $(BUILD)/boot/boot.sdb --listing $(BUILD)/boot/boot.lst --map $(BUILD)/boot/boot.map $<

# ---------------------------------------------------------------- kernel
$(BUILD)/kernel/kernel.ld: layout/kernel.ld.in include/strata/layout.h
	@mkdir -p $(dir $@)
	sed -e 's/@KERNEL_BASE@/$(call layout,KERNEL_BASE)/' -e 's/@KERNEL_SIZE@/$(call layout,KERNEL_SIZE)/' $< > $@

$(BUILD)/obj/font.c: font/strata8x16.font $(ROOT)/scripts/font.py
	@mkdir -p $(dir $@)
	$(PYTHON) $(ROOT)/scripts/font.py -f $< --c $@

$(BUILD)/obj/font.c.s: $(BUILD)/obj/font.c
	@echo "  CC   $<"; $(CC) $(CFLAGS) -S $< -o $@

KERNEL_OBJS := $(call objs,$(KERNEL)) $(BUILD)/obj/font.c.s

$(BUILD)/kernel/kernel.bin: $(KERNEL_OBJS) $(BUILD)/kernel/kernel.ld | tools
	$(AS) -T $(BUILD)/kernel/kernel.ld -o $(BUILD)/kernel/kernel.elf --bin $@ \
	    --debug $(BUILD)/kernel/kernel.sdb --listing $(BUILD)/kernel/kernel.lst \
	    --map $(BUILD)/kernel/kernel.map $(KERNEL_OBJS)

$(BUILD)/system.img: $(BUILD)/kernel/kernel.bin | tools
	$(PACK) -o $@ --entry _start@$(BUILD)/kernel/kernel.sdb $<@$(call layout,KERNEL_BASE)

# ---------------------------------------------------------------- user programs
define PROGRAM
$(eval NAME := $(word 1,$(subst :, ,$(1))))
$(eval SLOT := $(word 2,$(subst :, ,$(1))))
$(BUILD)/user/$(NAME)/user.ld: layout/user.ld.in include/strata/layout.h
	@mkdir -p $$(dir $$@)
	sed -e 's/@BASE@/$(call layout,SLOT$(SLOT)_BASE)/' -e 's/@SIZE@/$(call layout,SLOT$(SLOT)_SIZE)/' $$< > $$@

$(BUILD)/user/$(NAME)/crt0.s: user/crt0.S include/strata/layout.h
	@mkdir -p $$(dir $$@)
	$(CC) $(CPPFLAGS) -DPROG_NAME='"$(NAME)"' -E -x assembler-with-cpp $$< -o $$@

$(BUILD)/user/$(NAME)/$(NAME).bin: $(BUILD)/user/$(NAME)/crt0.s $(call objs,user/$(NAME).c user/ulib.c $(LIB)) $(BUILD)/user/$(NAME)/user.ld | tools
	$(AS) -T $(BUILD)/user/$(NAME)/user.ld -o $(BUILD)/user/$(NAME)/$(NAME).elf --bin $$@ \
	    --debug $(BUILD)/user/$(NAME)/$(NAME).sdb --listing $(BUILD)/user/$(NAME)/$(NAME).lst \
	    --map $(BUILD)/user/$(NAME)/$(NAME).map \
	    $(BUILD)/user/$(NAME)/crt0.s $(call objs,user/$(NAME).c user/ulib.c $(LIB))

PROGRAM_BINS += $(BUILD)/user/$(NAME)/$(NAME).bin
STORE_ARGS += $(NAME)=$(BUILD)/user/$(NAME)/$(NAME).bin@$(call layout,SLOT$(SLOT)_BASE):$(call layout,SLOT$(SLOT)_SIZE)
endef
$(foreach p,$(PROGRAMS),$(eval $(call PROGRAM,$(p))))

# ---------------------------------------------------------------- flash contents
$(BUILD)/flash.bin: $(BUILD)/system.img $(PROGRAM_BINS) $(ROOT)/scripts/mkflash.py
	$(PYTHON) $(ROOT)/scripts/mkflash.py -o $@ --image $(BUILD)/system.img $(STORE_ARGS)

# ---------------------------------------------------------------- tools
tools: $(AS) $(PACK)
$(AS) $(PACK):
	cd $(ROOT)/tools && cargo build --release -p strata-asm

clean:
	rm -rf $(BUILD)

.PHONY: all clean tools
.SECONDARY:
