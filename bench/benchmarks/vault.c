/* vault: a reverse-engineering challenge binary. It accepts one passcode and nothing else.
 * The passcode is not stored; it is only recoverable by reversing the three checks below. */
#include <stdio.h>
#include <string.h>

static const unsigned char vk[6]  = { 0x5a, 0x13, 0x7e, 0x22, 0x44, 0x6b };
static const unsigned char tgt[12] = { 0x6d, 0x41, 0x04, 0x06, 0x79, 0x1e, 0x08, 0x2a, 0x55, 0x48, 0x11, 0x54 };

/* stage 1: the passcode is exactly 12 bytes long. */
static int check_len(const char *s) {
    return strlen(s) == 12;
}

/* stage 2: per position i, ((s[i] + i) XOR vk[i % 6]) must equal tgt[i]. */
static int check_transform(const char *s) {
    for (int i = 0; i < 12; i++)
        if ((unsigned char)(((unsigned char)s[i] + i) ^ vk[i % 6]) != tgt[i])
            return 0;
    return 1;
}

/* stage 3: a weighted checksum over the passcode must match. */
static int check_sum(const char *s) {
    unsigned int sum = 0;
    for (int i = 0; i < 12; i++)
        sum += (unsigned int)(unsigned char)s[i] * (unsigned int)(i + 1);
    return sum == 5332u;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        printf("usage: %s <passcode>\n", argv[0]);
        return 2;
    }
    const char *p = argv[1];
    if (check_len(p) && check_transform(p) && check_sum(p)) {
        printf("Access granted. Vault unlocked.\n");
        return 0;
    }
    printf("Access denied.\n");
    return 1;
}
