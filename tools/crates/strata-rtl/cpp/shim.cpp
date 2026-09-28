// C interface to the Verilator model of strata_soc. Port order matches
// SOC_INPUTS and SOC_OUTPUTS in strata-sim/src/ports.rs (checked by a test).
#include <cstdint>
#include "Vstrata_soc.h"
#include "verilated.h"

struct Rtl {
    VerilatedContext ctx;
    Vstrata_soc* top;
};

static const char* const INPUT_NAMES[] = {
    "rst_in", "uart_rx", "btn", "spi_miso",
    "rom_a_rdata", "rom_b_rdata", "ram_a_rdata", "ram_b_rdata",
    "vram_a_rdata", "vram_b_rdata", "font_a_rdata", "font_b_rdata",
};

static const char* const OUTPUT_NAMES[] = {
    "uart_tx", "led", "spi_cs_n", "spi_sck", "spi_mosi", "vid_r", "vid_g", "vid_b", "vid_hs", "vid_vs",
    "vid_de", "halted", "halt_code", "rom_a_en", "rom_a_addr", "rom_b_en", "rom_b_addr", "ram_a_en",
    "ram_a_addr", "ram_b_en", "ram_b_we", "ram_b_addr", "ram_b_wdata", "vram_a_en", "vram_a_we",
    "vram_a_addr", "vram_a_wdata", "vram_b_en", "vram_b_addr", "font_a_en", "font_a_we", "font_a_addr",
    "font_a_wdata", "font_b_en", "font_b_addr", "trace_valid", "trace_trap", "trace_intr", "trace_cause",
    "trace_pc", "trace_insn", "trace_next_pc", "trace_mode", "trace_rd_we", "trace_rd", "trace_rd_wdata",
};

// Verilator references this for $time; the model has no timing statements.
double sc_time_stamp() { return 0; }

extern "C" {

int strata_rtl_available() { return 1; }

const char* strata_rtl_input_name(int i) {
    return i >= 0 && i < (int)(sizeof INPUT_NAMES / sizeof *INPUT_NAMES) ? INPUT_NAMES[i] : nullptr;
}

const char* strata_rtl_output_name(int i) {
    return i >= 0 && i < (int)(sizeof OUTPUT_NAMES / sizeof *OUTPUT_NAMES) ? OUTPUT_NAMES[i] : nullptr;
}

void* strata_rtl_new() {
    Rtl* r = new Rtl;
    r->top = new Vstrata_soc{&r->ctx};
    r->top->clk = 0;
    r->top->eval();
    return r;
}

void strata_rtl_free(void* p) {
    Rtl* r = static_cast<Rtl*>(p);
    r->top->final();
    delete r->top;
    delete r;
}

void strata_rtl_set_inputs(void* p, const uint64_t* v) {
    Vstrata_soc* t = static_cast<Rtl*>(p)->top;
    t->rst_in = v[0];
    t->uart_rx = v[1];
    t->btn = v[2];
    t->spi_miso = v[3];
    t->rom_a_rdata = v[4];
    t->rom_b_rdata = v[5];
    t->ram_a_rdata = v[6];
    t->ram_b_rdata = v[7];
    t->vram_a_rdata = v[8];
    t->vram_b_rdata = v[9];
    t->font_a_rdata = v[10];
    t->font_b_rdata = v[11];
}

void strata_rtl_eval(void* p) {
    Vstrata_soc* t = static_cast<Rtl*>(p)->top;
    t->clk = 0;
    t->eval();
}

void strata_rtl_tick(void* p) {
    Rtl* r = static_cast<Rtl*>(p);
    r->top->clk = 1;
    r->top->eval();
    r->ctx.timeInc(1);
}

void strata_rtl_outputs(void* p, uint64_t* o) {
    const Vstrata_soc* t = static_cast<Rtl*>(p)->top;
    int i = 0;
    o[i++] = t->uart_tx;      o[i++] = t->led;          o[i++] = t->spi_cs_n;     o[i++] = t->spi_sck;
    o[i++] = t->spi_mosi;     o[i++] = t->vid_r;        o[i++] = t->vid_g;        o[i++] = t->vid_b;
    o[i++] = t->vid_hs;       o[i++] = t->vid_vs;       o[i++] = t->vid_de;       o[i++] = t->halted;
    o[i++] = t->halt_code;    o[i++] = t->rom_a_en;     o[i++] = t->rom_a_addr;   o[i++] = t->rom_b_en;
    o[i++] = t->rom_b_addr;   o[i++] = t->ram_a_en;     o[i++] = t->ram_a_addr;   o[i++] = t->ram_b_en;
    o[i++] = t->ram_b_we;     o[i++] = t->ram_b_addr;   o[i++] = t->ram_b_wdata;  o[i++] = t->vram_a_en;
    o[i++] = t->vram_a_we;    o[i++] = t->vram_a_addr;  o[i++] = t->vram_a_wdata; o[i++] = t->vram_b_en;
    o[i++] = t->vram_b_addr;  o[i++] = t->font_a_en;    o[i++] = t->font_a_we;    o[i++] = t->font_a_addr;
    o[i++] = t->font_a_wdata; o[i++] = t->font_b_en;    o[i++] = t->font_b_addr;  o[i++] = t->trace_valid;
    o[i++] = t->trace_trap;   o[i++] = t->trace_intr;   o[i++] = t->trace_cause;  o[i++] = t->trace_pc;
    o[i++] = t->trace_insn;   o[i++] = t->trace_next_pc; o[i++] = t->trace_mode;  o[i++] = t->trace_rd_we;
    o[i++] = t->trace_rd;     o[i++] = t->trace_rd_wdata;
}

}  // extern "C"
