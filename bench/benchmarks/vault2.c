/* vault2: a harder reverse-engineering challenge. One passcode is accepted; recover it by
 * reversing the two-round, chained transform below. The passcode is not stored in the binary. */
#include <stdio.h>
#include <string.h>

static const unsigned char rk[5]   = { 0x3b, 0x9d, 0x17, 0xc4, 0x62 };
static const unsigned char tg[16]  = { 0xa9, 0x53, 0xa0, 0x89, 0x98, 0xa1, 0x72, 0xae, 0x62, 0xbd, 0x20, 0xc0, 0x25, 0x15, 0x3e, 0x8d };
static const unsigned char seed    = 0x5f;

static int check_len(const char *s) { return strlen(s) == 16; }

/* round 1: XOR with the rolling key. round 2: chain each byte into the previous result. */
static int check_chain(const char *s) {
    unsigned char prev = seed;
    for (int i = 0; i < 16; i++) {
        unsigned char b = (unsigned char)s[i] ^ rk[i % 5];
        unsigned char c = (unsigned char)(b + prev);
        if (c != tg[i]) return 0;
        prev = c;
    }
    return 1;
}

static int check_sum(const char *s) {
    unsigned int sum = 0;
    for (int i = 0; i < 16; i++) sum += (unsigned int)(unsigned char)s[i] * (unsigned int)(i + 3);
    return sum == 13375u;
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: %s <passcode>\n", argv[0]); return 2; }
    const char *p = argv[1];
    if (check_len(p) && check_chain(p) && check_sum(p)) {
        printf("Access granted. Vault unlocked.\n"); return 0;
    }
    printf("Access denied.\n"); return 1;
}
