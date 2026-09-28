/* Built when Verilator is not available: the RTL engine reports itself missing. */
#include <stddef.h>
#include <stdint.h>
int strata_rtl_available(void) { return 0; }
const char* strata_rtl_input_name(int i) { (void)i; return NULL; }
const char* strata_rtl_output_name(int i) { (void)i; return NULL; }
void* strata_rtl_new(void) { return NULL; }
void strata_rtl_free(void* p) { (void)p; }
void strata_rtl_set_inputs(void* p, const uint64_t* v) { (void)p; (void)v; }
void strata_rtl_eval(void* p) { (void)p; }
void strata_rtl_tick(void* p) { (void)p; }
void strata_rtl_outputs(void* p, uint64_t* o) { (void)p; (void)o; }
