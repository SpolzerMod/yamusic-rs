// Регрессия: расширение делегата не должно менять объект/ivars и ломать методы winit.
#import <Cocoa/Cocoa.h>
#import <MediaPlayer/MediaPlayer.h>
#import <objc/runtime.h>
#include <assert.h>
#include <stdint.h>
extern bool ym_start(void (*)(int32_t, double));
extern void ym_stop(void);
extern void ym_media_update(const uint8_t *, size_t);
extern int32_t ym_autostart_status(void);
static int lastCommand = -1;
static void received(int32_t command, double value) { lastCommand = command; }
@interface WinitApplicationDelegate : NSObject <NSApplicationDelegate>
@property NSInteger launched;
@end
@implementation WinitApplicationDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)note { self.launched++; }
@end
int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        WinitApplicationDelegate *original = [WinitApplicationDelegate new];
        NSApp.delegate = original;
        Class originalClass = object_getClass(original);
        assert(ym_start(received));
        assert(NSApp.delegate == original);
        assert([NSApp.delegate isKindOfClass:WinitApplicationDelegate.class]);
        [NSApp.delegate applicationDidFinishLaunching:[NSNotification notificationWithName:NSApplicationDidFinishLaunchingNotification object:NSApp]];
        assert(original.launched == 1);
        assert(![NSApp.delegate applicationShouldHandleReopen:NSApp hasVisibleWindows:NO]);
        assert(lastCommand == 0);
        assert(![NSApp.delegate applicationShouldTerminateAfterLastWindowClosed:NSApp]);
        assert(!ym_start(received));
        assert(ym_autostart_status() == -1); // тест вне .app не регистрирует автозапуск
        const char *snapshot = "{\"id\":\"native-test\",\"title\":\"Test\",\"duration\":120,\"position\":12,\"playing\":false}";
        ym_media_update((const uint8_t *)snapshot, strlen(snapshot));
        assert([MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo[MPMediaItemPropertyTitle] isEqualToString:@"Test"]);
        assert([MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo[MPNowPlayingInfoPropertyElapsedPlaybackTime] doubleValue] == 12);
        assert(MPNowPlayingInfoCenter.defaultCenter.playbackState == MPNowPlayingPlaybackStatePaused);
        ym_stop();
        assert(MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo == nil);
        assert(NSApp.delegate == original);
        assert(object_getClass(original) == originalClass);
        assert(ym_start(received)); // повторная сессия использует зарегистрированный подкласс
        ym_stop();
        puts("macOS native bridge: passed");
    }
    return 0;
}
