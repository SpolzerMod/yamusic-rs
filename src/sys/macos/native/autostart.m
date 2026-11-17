#import <Cocoa/Cocoa.h>
#import <ServiceManagement/ServiceManagement.h>
#include <stdint.h>

static BOOL installedBundle(void) {
    NSString *path = NSBundle.mainBundle.bundlePath.stringByStandardizingPath;
    NSString *userApps = [NSHomeDirectory() stringByAppendingPathComponent:@"Applications/"];
    return [path.pathExtension isEqualToString:@"app"]
        && ([path hasPrefix:@"/Applications/"] || [path hasPrefix:userApps]);
}
int32_t ym_autostart_status(void) {
    if (!installedBundle()) return -1;
    return (int32_t)SMAppService.mainAppService.status;
}
bool ym_autostart_set(bool enabled, char *errorBuffer, size_t capacity) {
    if (!installedBundle()) {
        strlcpy(errorBuffer, "Переместите приложение в папку «Программы» или ~/Applications", capacity);
        return false;
    }
    NSError *error = nil;
    BOOL ok = enabled ? [SMAppService.mainAppService registerAndReturnError:&error]
                      : [SMAppService.mainAppService unregisterAndReturnError:&error];
    if (!ok) strlcpy(errorBuffer, error.localizedDescription.UTF8String ?: "ServiceManagement error", capacity);
    return ok;
}
void ym_autostart_settings(void) { [SMAppService openSystemSettingsLoginItems]; }
