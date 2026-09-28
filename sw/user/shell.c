/* The Strata shell. */
#include <ctype.h>
#include <string.h>
#include <strata/user.h>

#define LINE_MAX 72

static const char *const state_names[] = {"free", "ready", "sleep", "input", "vsync", "wait", "exit"};

static char line[LINE_MAX + 1];
static char prev[LINE_MAX + 1];

/* Reads a line with basic editing: backspace, Ctrl-U, up-arrow to recall
 * the previous line, and Ctrl-C to discard the line. */
static void read_line(void)
{
    size_t n = 0;
    for (;;) {
        int k = sys_getkey(1);
        if (k == '\n') {
            puts_("\n");
            line[n] = 0;
            return;
        }
        if (k == 0x03) {
            puts_("^C\n");
            line[0] = 0;
            return;
        }
        if ((k == '\b' || k == 0x7F) && n > 0) {
            n--;
            puts_("\b \b");
        } else if (k == 0x15 || k == KEY_UP) {
            while (n > 0) {
                n--;
                puts_("\b \b");
            }
            if (k == KEY_UP) {
                strcpy(line, prev);
                n = strlen(line);
                puts_(line);
            }
        } else if (k >= 32 && k < 127 && n < LINE_MAX) {
            line[n++] = (char)k;
            putchar_(k);
        }
    }
}

static char *next_word(char **p)
{
    while (**p == ' ')
        (*p)++;
    if (!**p)
        return 0;
    char *w = *p;
    while (**p && **p != ' ')
        (*p)++;
    if (**p)
        *(*p)++ = 0;
    return w;
}

static uint32_t parse_num(const char *s)
{
    uint32_t v = 0;
    if (s[0] == '0' && (s[1] | 32) == 'x') {
        for (s += 2; isxdigit(*s); s++)
            v = v * 16 + (uint32_t)(isdigit(*s) ? *s - '0' : (*s | 32) - 'a' + 10);
    } else {
        for (; isdigit(*s); s++)
            v = v * 10 + (uint32_t)(*s - '0');
    }
    return v;
}

static void cmd_help(void)
{
    puts_("commands:\n"
          "  help                 this list\n"
          "  ls                   programs in the store\n"
          "  <program> [arg] [&]  run a program; '&' runs it in the background\n"
          "  ps                   tasks\n"
          "  kill <pid>           stop a task\n"
          "  info                 machine and performance counters\n"
          "  leds <n>             set the LEDs\n"
          "  echo <text>          print text\n"
          "  clear                clear the screen\n"
          "  sleep <ms>           wait\n"
          "  halt [code]          stop the machine\n"
          "  reboot               reset the machine\n"
          "Ctrl-C stops the program running in the foreground.\n");
}

static void cmd_ls(void)
{
    char name[16];
    for (int i = 0; sys_programs(i, name) == 0; i++)
        printf("  %s\n", name);
}

static void cmd_ps(void)
{
    struct ps_entry e[8];
    int n = sys_ps(e, 8);
    puts_("  pid  state  slot  cycles       name\n");
    for (int i = 0; i < n; i++) {
        unsigned long long c = ((unsigned long long)e[i].cycles_hi << 32) | e[i].cycles_lo;
        printf("  %3d  %-5s  %4d  %-11llu  %s\n", e[i].pid, state_names[e[i].state], e[i].slot, c, e[i].name);
    }
}

static void cmd_info(void)
{
    struct sys_info s;
    sys_info(&s);
    unsigned long long cyc = ((unsigned long long)s.cycle_hi << 32) | s.cycle_lo;
    unsigned long long ins = ((unsigned long long)s.instret_hi << 32) | s.instret_lo;
    unsigned cpi100 = ins ? (unsigned)(cyc * 100 / ins) : 0;
    printf("  clock        %u Hz\n", (unsigned)s.clk_hz);
    printf("  RAM          %u bytes\n", (unsigned)s.ram_bytes);
    printf("  flash id     %06x\n", (unsigned)s.flash_id);
    printf("  uptime       %u ms\n", (unsigned)s.time_ms);
    printf("  video frames %u\n", (unsigned)s.frames);
    printf("  cycles       %llu\n", cyc);
    printf("  instret      %llu\n", ins);
    printf("  CPI          %u.%02u\n", cpi100 / 100, cpi100 % 100);
}

/* Runs a program. A numeric argument is passed as its value; a word is
 * passed as its first character with bit 31 set. */
static void run(const char *prog, char *rest)
{
    uint32_t arg = 0;
    int background = 0;
    char *w;
    while ((w = next_word(&rest)) != 0) {
        if (strcmp(w, "&") == 0)
            background = 1;
        else
            arg = isdigit((unsigned char)w[0]) ? parse_num(w) : (0x80000000u | (uint8_t)w[0]);
    }
    int pid = sys_spawn(prog, arg);
    if (pid == E_NOENT) {
        printf("%s: unknown command or program (try 'help')\n", prog);
        return;
    }
    if (pid < 0) {
        printf("%s: cannot start (%s)\n", prog, pid == E_BUSY ? "its slot is in use" : "bad program");
        return;
    }
    if (background) {
        printf("[%d] %s\n", pid, prog);
        return;
    }
    int code = sys_wait(pid);
    if (code != 0 && code != EXIT_INTERRUPTED)
        printf("%s exited with %d\n", prog, code);
}

static void execute(char *p)
{
    char *cmd = next_word(&p);
    if (!cmd)
        return;
    if (strcmp(cmd, "help") == 0) {
        cmd_help();
    } else if (strcmp(cmd, "ls") == 0) {
        cmd_ls();
    } else if (strcmp(cmd, "ps") == 0) {
        cmd_ps();
    } else if (strcmp(cmd, "kill") == 0) {
        char *a = next_word(&p);
        if (!a || sys_kill((int)parse_num(a)) < 0)
            puts_("kill: no such task\n");
    } else if (strcmp(cmd, "info") == 0) {
        cmd_info();
    } else if (strcmp(cmd, "leds") == 0) {
        char *a = next_word(&p);
        sys_leds(a ? parse_num(a) : 0);
    } else if (strcmp(cmd, "echo") == 0) {
        printf("%s\n", p);
    } else if (strcmp(cmd, "clear") == 0) {
        puts_("\x1b[2J\x1b[H");
    } else if (strcmp(cmd, "sleep") == 0) {
        char *a = next_word(&p);
        sys_sleep(a ? parse_num(a) : 1000);
    } else if (strcmp(cmd, "halt") == 0) {
        char *a = next_word(&p);
        sys_halt(a ? (int)parse_num(a) : 0);
    } else if (strcmp(cmd, "reboot") == 0) {
        sys_reset();
    } else {
        run(cmd, p);
    }
}

int main(void)
{
    puts_("Type 'help' for commands.\n");
    for (;;) {
        puts_("\x1b[92mstrata\x1b[0m> ");
        read_line();
        if (line[0])
            strcpy(prev, line);
        execute(line);
    }
}
