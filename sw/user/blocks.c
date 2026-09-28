/*
 * Strata Blocks: a falling-block puzzle for the text display.
 *
 *   blocks          play: arrows or buttons move, Up/A/x rotate, z rotates
 *                   back, Down drops faster, B/space drops, START/p pauses,
 *                   q quits
 *   blocks demo     the computer plays until a key is pressed
 *   blocks <n>      benchmark: the computer places n pieces as fast as it can
 *                   (no frame pacing), prints the result and exits with the
 *                   score; used by the whole-machine tests
 *
 * The board and panels are drawn directly into text VRAM, which the kernel
 * maps for this task with SYS_SCREEN. Game time is counted in video frames
 * (SYS_VSYNC), so the game runs at the same pace whatever the CPU speed.
 */
#include <string.h>
#include <strata/user.h>

#define W 10
#define H 20
#define BOARD_X 30        /* screen column of the board's left edge */
#define BOARD_Y 5         /* screen row of the board's top edge */
#define PANEL_X 55

#define ARG_WORD 0x80000000u

enum { BLACK, BLUE, GREEN, CYAN, RED, MAGENTA, BROWN, GREY,
       DARK, LBLUE, LGREEN, LCYAN, LRED, LMAGENTA, YELLOW, WHITE };

#define CH_BLOCK   0xDB
#define CH_DOT     0xF9
#define CH_DV      0xBA
#define CH_DH      0xCD
#define CH_DTL     0xC9
#define CH_DTR     0xBB
#define CH_DBL     0xC8
#define CH_DBR     0xBC

/* Tetrominoes, 4 rotations each, as 4x4 masks: bit 15 is the top-left cell. */
static const uint16_t shapes[7][4] = {
    {0x0F00, 0x2222, 0x00F0, 0x4444},   /* I */
    {0x6600, 0x6600, 0x6600, 0x6600},   /* O */
    {0x4E00, 0x4640, 0x0E40, 0x4C40},   /* T */
    {0x6C00, 0x4620, 0x06C0, 0x8C40},   /* S */
    {0xC600, 0x2640, 0x0C60, 0x4C80},   /* Z */
    {0x8E00, 0x6440, 0x0E20, 0x44C0},   /* J */
    {0x2E00, 0x4460, 0x0E80, 0xC440},   /* L */
};
static const uint8_t colors[7] = {LCYAN, YELLOW, LMAGENTA, LGREEN, LRED, LBLUE, BROWN};
static const uint8_t gravity[] = {48, 43, 38, 33, 28, 23, 18, 13, 8, 6, 5, 5, 5, 4, 4, 4, 3, 3, 3, 2};
static const uint16_t line_score[5] = {0, 100, 300, 500, 800};

static volatile uint16_t *vram;
static uint8_t board[H][W];          /* 0 empty, else piece color */
static int piece, rot, px, py, next_piece;
static uint32_t score, lines, level, pieces;
static uint32_t rng;
static uint8_t bag[7];
static int bag_left;

/* ---------------------------------------------------------------- drawing */

static void put(int r, int c, int ch, int fg, int bg)
{
    vram[r * 128 + c] = (uint16_t)((bg << 12) | (fg << 8) | (ch & 0xFF));
}

static void text(int r, int c, const char *s, int fg)
{
    for (; *s; s++)
        put(r, c++, (uint8_t)*s, fg, BLACK);
}

static void number(int r, int c, uint32_t v, int width, int fg)
{
    char buf[12];
    int n = 0;
    do {
        buf[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v && n < 11);
    for (int i = 0; i < width - n; i++)
        put(r, c++, ' ', fg, BLACK);
    while (n)
        put(r, c++, buf[--n], fg, BLACK);
}

static void frame_box(int r0, int c0, int r1, int c1, int fg)
{
    for (int c = c0 + 1; c < c1; c++) {
        put(r0, c, CH_DH, fg, BLACK);
        put(r1, c, CH_DH, fg, BLACK);
    }
    for (int r = r0 + 1; r < r1; r++) {
        put(r, c0, CH_DV, fg, BLACK);
        put(r, c1, CH_DV, fg, BLACK);
    }
    put(r0, c0, CH_DTL, fg, BLACK);
    put(r0, c1, CH_DTR, fg, BLACK);
    put(r1, c0, CH_DBL, fg, BLACK);
    put(r1, c1, CH_DBR, fg, BLACK);
}

static void draw_cell(int y, int x, int color)
{
    int r = BOARD_Y + y, c = BOARD_X + 2 * x;
    if (color) {
        put(r, c, CH_BLOCK, color, BLACK);
        put(r, c + 1, CH_BLOCK, color, BLACK);
    } else {
        put(r, c, CH_DOT, DARK, BLACK);
        put(r, c + 1, ' ', DARK, BLACK);
    }
}

static int cell_set(uint16_t mask, int cx, int cy)
{
    return (mask >> (15 - (cy * 4 + cx))) & 1;
}

static void draw_board(void)
{
    uint16_t m = shapes[piece][rot];
    for (int y = 0; y < H; y++) {
        for (int x = 0; x < W; x++) {
            int color = board[y][x];
            int cx = x - px, cy = y - py;
            if (cx >= 0 && cx < 4 && cy >= 0 && cy < 4 && cell_set(m, cx, cy))
                color = colors[piece];
            draw_cell(y, x, color);
        }
    }
}

static void draw_panel(void)
{
    uint16_t m = shapes[next_piece][0];
    for (int cy = 0; cy < 4; cy++)
        for (int cx = 0; cx < 4; cx++) {
            int on = cell_set(m, cx, cy);
            put(7 + cy, PANEL_X + 2 + 2 * cx, on ? CH_BLOCK : ' ', colors[next_piece], BLACK);
            put(7 + cy, PANEL_X + 3 + 2 * cx, on ? CH_BLOCK : ' ', colors[next_piece], BLACK);
        }
    number(14, PANEL_X + 2, score, 8, WHITE);
    number(17, PANEL_X + 2, lines, 8, WHITE);
    number(20, PANEL_X + 2, level, 8, WHITE);
}

static void draw_frame(void)
{
    for (int r = 0; r < 30; r++)
        for (int c = 0; c < 80; c++)
            put(r, c, ' ', GREY, BLACK);
    text(1, 27, "S T R A T A   B L O C K S", WHITE);
    frame_box(BOARD_Y - 1, BOARD_X - 1, BOARD_Y + H, BOARD_X + 2 * W, LCYAN);
    frame_box(5, PANEL_X, 12, PANEL_X + 11, GREY);
    text(5, PANEL_X + 2, " NEXT ", WHITE);
    text(13, PANEL_X + 2, "SCORE", GREY);
    text(16, PANEL_X + 2, "LINES", GREY);
    text(19, PANEL_X + 2, "LEVEL", GREY);
    static const char *const help[] = {
        "CONTROLS", "", "\x1B \x1A   move", "\x18 A x  rotate", "z      rotate back",
        "\x19      faster", "B spc  drop", "START p pause", "q      quit",
    };
    for (unsigned i = 0; i < sizeof help / sizeof help[0]; i++)
        text(6 + (int)i, 4, help[i], i ? GREY : WHITE);
}

static void message(const char *s)
{
    int len = (int)strlen(s), c = BOARD_X + W - len / 2;
    frame_box(12, c - 2, 16, c + len + 1, WHITE);
    for (int r = 13; r < 16; r++)
        for (int x = c - 1; x <= c + len; x++)
            put(r, x, ' ', WHITE, BLACK);
    text(14, c, s, YELLOW);
}

/* ---------------------------------------------------------------- rules */

static uint32_t rand32(void)
{
    rng ^= rng << 13;
    rng ^= rng >> 17;
    rng ^= rng << 5;
    return rng;
}

/* 7-bag randomizer: every piece once per 7 draws, in random order. */
static int draw_piece(void)
{
    if (bag_left == 0) {
        for (int i = 0; i < 7; i++)
            bag[i] = (uint8_t)i;
        for (int i = 6; i > 0; i--) {
            int j = (int)(rand32() % (uint32_t)(i + 1));
            uint8_t t = bag[i];
            bag[i] = bag[j];
            bag[j] = t;
        }
        bag_left = 7;
    }
    return bag[--bag_left];
}

static int fits(int p, int r, int x, int y)
{
    uint16_t m = shapes[p][r];
    for (int cy = 0; cy < 4; cy++)
        for (int cx = 0; cx < 4; cx++) {
            if (!cell_set(m, cx, cy))
                continue;
            int bx = x + cx, by = y + cy;
            if (bx < 0 || bx >= W || by >= H)
                return 0;
            if (by >= 0 && board[by][bx])
                return 0;
        }
    return 1;
}

static int spawn(void)
{
    piece = next_piece;
    next_piece = draw_piece();
    rot = 0;
    px = 3;
    py = piece == 0 ? -1 : 0;
    pieces++;
    return fits(piece, rot, px, py);
}

static void rotate(int dir)
{
    int r = (rot + dir) & 3;
    static const int kicks[] = {0, -1, 1, -2, 2};
    for (unsigned i = 0; i < sizeof kicks / sizeof kicks[0]; i++) {
        if (fits(piece, r, px + kicks[i], py)) {
            rot = r;
            px += kicks[i];
            return;
        }
    }
}

/* Locks the piece into the board and clears full rows; returns rows cleared. */
static int lock_piece(void)
{
    uint16_t m = shapes[piece][rot];
    for (int cy = 0; cy < 4; cy++)
        for (int cx = 0; cx < 4; cx++)
            if (cell_set(m, cx, cy) && py + cy >= 0)
                board[py + cy][px + cx] = colors[piece];
    int cleared = 0;
    for (int y = H - 1; y >= 0; y--) {
        int full = 1;
        for (int x = 0; x < W; x++)
            full &= board[y][x] != 0;
        if (full) {
            memmove(board[1], board[0], (size_t)y * W);
            memset(board[0], 0, W);
            cleared++;
            y++;
        }
    }
    lines += (uint32_t)cleared;
    score += line_score[cleared] * level;
    level = 1 + lines / 10;
    return cleared;
}

static int drop_distance(void)
{
    int d = 0;
    while (fits(piece, rot, px, py + d + 1))
        d++;
    return d;
}

/* ---------------------------------------------------------------- computer player */

/* Scores the board after a placement (higher is better): fewer holes, lower
 * and flatter stacks, more cleared rows. */
static int evaluate(const uint8_t b[H][W], int cleared)
{
    int heights[W], holes = 0, total = 0, bump = 0;
    for (int x = 0; x < W; x++) {
        int h = 0;
        for (int y = 0; y < H; y++)
            if (b[y][x]) {
                h = H - y;
                break;
            }
        heights[x] = h;
        total += h;
        for (int y = H - h + 1; y < H; y++)
            if (!b[y][x])
                holes++;
    }
    for (int x = 0; x + 1 < W; x++)
        bump += heights[x] > heights[x + 1] ? heights[x] - heights[x + 1] : heights[x + 1] - heights[x];
    return 76 * cleared - 51 * total - 36 * holes - 18 * bump;
}

static void best_move(int *best_r, int *best_x)
{
    static uint8_t trial[H][W];
    int best = -0x7FFFFFFF;
    *best_r = rot;
    *best_x = px;
    for (int r = 0; r < 4; r++) {
        for (int x = -2; x < W; x++) {
            if (!fits(piece, r, x, py))
                continue;
            int y = py;
            while (fits(piece, r, x, y + 1))
                y++;
            memcpy(trial, board, sizeof trial);
            uint16_t m = shapes[piece][r];
            for (int cy = 0; cy < 4; cy++)
                for (int cx = 0; cx < 4; cx++)
                    if (cell_set(m, cx, cy) && y + cy >= 0)
                        trial[y + cy][x + cx] = 1;
            int cleared = 0;
            for (int yy = 0; yy < H; yy++) {
                int full = 1;
                for (int xx = 0; xx < W; xx++)
                    full &= trial[yy][xx] != 0;
                cleared += full;
            }
            int s = evaluate((const uint8_t (*)[W])trial, cleared);
            if (s > best) {
                best = s;
                *best_r = r;
                *best_x = x;
            }
        }
    }
}

/* ---------------------------------------------------------------- modes */

static void new_game(uint32_t seed)
{
    memset(board, 0, sizeof board);
    score = lines = pieces = 0;
    level = 1;
    rng = seed ? seed : 1;
    bag_left = 0;
    next_piece = draw_piece();
    spawn();
}

/* Places pieces with the computer player as fast as possible. */
static int bench(uint32_t n)
{
    uint32_t t0 = sys_time();
    new_game(0x5EED);
    while (pieces <= n) {
        int r, x;
        best_move(&r, &x);
        rot = r;
        px = x;
        py += drop_distance();
        lock_piece();
        if (!spawn())
            break;
    }
    uint32_t placed = pieces - 1;
    printf("blocks: %u pieces, %u lines, score %u, %u ms\n", (unsigned)placed, (unsigned)lines, (unsigned)score,
           (unsigned)(sys_time() - t0));
    return (int)score;
}

static int play(int demo)
{
    vram = sys_screen(1);
    if ((int32_t)(uintptr_t)vram < 0) {
        puts_("blocks: the screen is in use\n");
        return 1;
    }
    new_game(demo ? 0x5EED : sys_time() ^ (sys_vsync() << 16));
    draw_frame();
    int over = 0, paused = 0, fall = 0, demo_step = 0;
    uint32_t frame = sys_vsync();
    for (;;) {
        int quit = 0, k;
        while ((k = sys_getkey(0)) >= 0) {
            if (demo || k == 'q') {
                quit = 1;
                break;
            }
            if (over) {
                if (k == '\n' || k == ' ' || k == KEY_START || k == KEY_A)
                    quit = 1;
                continue;
            }
            if (k == KEY_START || k == 'p') {
                paused = !paused;
                if (paused)
                    message("PAUSED");
                else
                    draw_frame();
                continue;
            }
            if (paused)
                continue;
            switch (k) {
            case KEY_LEFT: if (fits(piece, rot, px - 1, py)) px--; break;
            case KEY_RIGHT: if (fits(piece, rot, px + 1, py)) px++; break;
            case KEY_UP: case KEY_A: case 'x': rotate(1); break;
            case 'z': rotate(-1); break;
            case KEY_DOWN:
                if (fits(piece, rot, px, py + 1)) {
                    py++;
                    score += 1;
                    fall = 0;
                }
                break;
            case KEY_B: case ' ': {
                int d = drop_distance();
                py += d;
                score += 2 * (uint32_t)d;
                fall = 1000;
                break;
            }
            default: break;
            }
        }
        if (quit)
            break;

        uint32_t now = sys_vsync();
        int elapsed = (int)(now - frame);
        frame = now;
        if (over || paused)
            continue;

        if (demo && ++demo_step % 4 == 0) {
            int r, x;
            best_move(&r, &x);
            if (rot != r)
                rotate(1);
            else if (px < x && fits(piece, rot, px + 1, py))
                px++;
            else if (px > x && fits(piece, rot, px - 1, py))
                px--;
            else
                fall = 1000;
        }

        fall += elapsed;
        int speed = gravity[level - 1 < sizeof gravity ? level - 1 : sizeof gravity - 1];
        if (fall >= speed) {
            fall = 0;
            if (fits(piece, rot, px, py + 1)) {
                py++;
            } else {
                lock_piece();
                if (!spawn()) {
                    over = 1;
                    draw_board();
                    draw_panel();
                    message(demo ? "DEMO OVER" : "GAME OVER");
                    if (demo)
                        break;
                    continue;
                }
            }
        }
        draw_board();
        draw_panel();
    }
    sys_screen(0);
    printf("blocks: score %u, %u lines, level %u\n", (unsigned)score, (unsigned)lines, (unsigned)level);
    return 0;
}

int main(int argument)
{
    uint32_t arg = (uint32_t)argument;
    if (arg & ARG_WORD)
        return play((arg & 0xFF) == 'd');
    if (arg)
        return bench(arg);
    return play(0);
}
