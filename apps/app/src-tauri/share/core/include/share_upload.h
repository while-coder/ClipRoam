#ifndef CLIPROAM_SHARE_UPLOAD_H
#define CLIPROAM_SHARE_UPLOAD_H
#ifdef __cplusplus
extern "C" {
#endif
// JSON in/out. The caller owns the result and must release it with free.
char *cliproam_share_call(const char *input);
void cliproam_share_free(char *output);
#ifdef __cplusplus
}
#endif
#endif
