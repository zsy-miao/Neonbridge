/* Démonstration autonome. DrawFrame reçoit un tampon RGB de 160 x 96 pixels. */
__declspec(dllimport) unsigned XSystemGetDeviceType(void);

unsigned entry(void) {
    return XSystemGetDeviceType();
}

static int car_x = 80;

__declspec(dllexport) void DrawFrame(unsigned char *out, unsigned frame, int direction) {
    car_x += direction * 2;
    if (car_x < 19) car_x = 19;
    if (car_x > 141) car_x = 141;

    for (int y = 0; y < 96; ++y) {
        int depth = y - 40;
        for (int x = 0; x < 160; ++x) {
            int dx = x - 80;
            int dist = dx < 0 ? -dx : dx;
            unsigned rgb = 0x302653 + y * 0x030101;
            if (depth < 0) {
                int sx = x - 116, sy = y - 25;
                if (sx*sx + sy*sy < 144 && (y < 26 || y % 4 != 0)) rgb = 0xffbc71;
                if (y > 38 - ((x/7*13 + x/17*3) % 13)) rgb = 0x432c50;
            } else {
                int edge = 4 + depth;
                int stripe = ((depth*depth / 65 - (int)(frame % 512)) & 15) < 8;
                rgb = 0x38665e;
                if (dist <= edge + 2) rgb = stripe ? 0xfacd99 : 0xbf625f;
                if (dist <= edge) rgb = 0x303345;
                if (dist < 1 + depth/25 && stripe) rgb = 0xf4d394;
            }
            int cx = x - car_x;
            if (cx < 0) cx = -cx;
            if (y >= 72 && y < 89 && cx < (y < 76 ? 7 : 12)) {
                rgb = 0xde524e;
                if (y >= 74 && y <= 78 && cx < 7) rgb = 0x94ced0;
                if (y >= 82 && y <= 84 && cx >= 7) rgb = 0xffca68;
                if (y >= 86) rgb = 0x252232;
            }
            *out++ = (unsigned char)(rgb >> 16);
            *out++ = (unsigned char)(rgb >> 8);
            *out++ = (unsigned char)rgb;
        }
    }
}
