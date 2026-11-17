// AppKit работает только в главном потоке. Rust владеет временем жизни моста.
#import <Cocoa/Cocoa.h>
#import <objc/runtime.h>
#include <stdint.h>

typedef void (*YMCallback)(int32_t, double);
static YMCallback callback;
extern void ym_media_start(YMCallback);
extern void ym_media_stop(void);
static bool mediaHasTrack;
static bool miniMode;
static NSMenuItem *playItem;
static NSMenu *previousMenu;
static NSMenu *applicationMenu;
static id savedApplicationDelegate;
static Class originalDelegateClass;
static bool terminationPending;
static __weak NSWindow *playerWindow;
static id screensObserver;

static void sendCommand(int32_t command, double value) {
    if (callback) callback(command, value);
}

@interface YMApplicationDelegate : NSObject <NSMenuItemValidation>
@end
static YMApplicationDelegate *delegate;

@implementation YMApplicationDelegate
- (void)command:(NSMenuItem *)item { sendCommand((int32_t)item.tag, 0); }
- (void)about:(id)sender { [NSApp orderFrontStandardAboutPanel:sender]; }
- (void)fullscreen:(id)sender { if (!miniMode) [playerWindow toggleFullScreen:sender]; }
- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action == @selector(fullscreen:)) return playerWindow && !miniMode;
    if (item.action == @selector(command:) && item.tag == 10) return !(playerWindow.styleMask & NSWindowStyleMaskFullScreen);
    if (item.action == @selector(command:) && item.tag >= 11 && item.tag <= 14) return playerWindow.isKeyWindow;
    if (item.action == @selector(command:) && item.tag >= 4 && item.tag <= 9) return mediaHasTrack;
    return YES;
}
@end

// winit 0.30.13 требует, чтобы NSApp.delegate оставался экземпляром его класса.
// Добавляем методы подклассу того же объекта, сохраняя ivars, launch и terminate hooks.
static BOOL reopen(id self, SEL selector, NSApplication *app, BOOL visible) {
    sendCommand(0, 0); return NO;
}
static BOOL terminateAfterClose(id self, SEL selector, NSApplication *app) { return NO; }
static NSApplicationTerminateReply requestTermination(id self, SEL selector, NSApplication *app) {
    terminationPending = true;
    sendCommand(2, 0);
    return NSTerminateLater;
}
static bool extendWinitDelegate(void) {
    savedApplicationDelegate = NSApp.delegate;
    originalDelegateClass = object_getClass(savedApplicationDelegate);
    if (!originalDelegateClass || strcmp(class_getName(originalDelegateClass), "WinitApplicationDelegate") != 0) return false;
    Class extended = objc_lookUpClass("YMWinitApplicationDelegate");
    if (!extended) {
        extended = objc_allocateClassPair(originalDelegateClass, "YMWinitApplicationDelegate", 0);
        if (!extended) return false;
        NSString *reopenTypes = [NSString stringWithFormat:@"%s@:@%s", @encode(BOOL), @encode(BOOL)];
        NSString *boolTypes = [NSString stringWithFormat:@"%s@:@", @encode(BOOL)];
        NSString *terminateTypes = [NSString stringWithFormat:@"%s@:@", @encode(NSApplicationTerminateReply)];
        class_addMethod(extended, @selector(applicationShouldHandleReopen:hasVisibleWindows:), (IMP)reopen, reopenTypes.UTF8String);
        class_addMethod(extended, @selector(applicationShouldTerminateAfterLastWindowClosed:), (IMP)terminateAfterClose, boolTypes.UTF8String);
        class_addMethod(extended, @selector(applicationShouldTerminate:), (IMP)requestTermination, terminateTypes.UTF8String);
        objc_registerClassPair(extended);
    }
    if (class_getSuperclass(extended) != originalDelegateClass) return false;
    object_setClass(savedApplicationDelegate, extended);
    return true;
}

static NSMenu *submenu(NSMenu *bar, NSString *title) {
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:nil keyEquivalent:@""];
    NSMenu *menu = [[NSMenu alloc] initWithTitle:title];
    item.submenu = menu;
    [bar addItem:item];
    return menu;
}
static NSMenuItem *item(NSMenu *menu, NSString *title, SEL action, NSString *key, id target) {
    NSMenuItem *i = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:key];
    i.target = target;
    [menu addItem:i];
    return i;
}
static NSMenuItem *command(NSMenu *menu, NSString *title, NSString *key, NSInteger tag) {
    NSMenuItem *i = item(menu, title, @selector(command:), key, delegate);
    i.tag = tag;
    return i;
}
static void buildMenu(void) {
    NSMenu *bar = [[NSMenu alloc] init];
    NSMenu *app = submenu(bar, @"Яндекс Музыка RS");
    item(app, @"О программе «Яндекс Музыка RS»", @selector(about:), @"", delegate);
    [app addItem:NSMenuItem.separatorItem];
    command(app, @"Настройки…", @",", 3);
    [app addItem:NSMenuItem.separatorItem];
    item(app, @"Скрыть Яндекс Музыка RS", @selector(hide:), @"h", NSApp);
    NSMenuItem *hideOthers = item(app, @"Скрыть остальные", @selector(hideOtherApplications:), @"h", NSApp);
    hideOthers.keyEquivalentModifierMask = NSEventModifierFlagCommand | NSEventModifierFlagOption;
    item(app, @"Показать все", @selector(unhideAllApplications:), @"", NSApp);
    [app addItem:NSMenuItem.separatorItem];
    command(app, @"Завершить Яндекс Музыка RS", @"q", 2);
    NSMenu *edit = submenu(bar, @"Правка");
    // Ввод маршрутизируется в Slint через общий канал команд.
    command(edit, @"Вырезать", @"x", 11);
    command(edit, @"Копировать", @"c", 12);
    command(edit, @"Вставить", @"v", 13);
    command(edit, @"Выделить всё", @"a", 14);
    NSMenu *play = submenu(bar, @"Воспроизведение");
    playItem = command(play, @"Воспроизвести / Пауза", @"", 6);
    command(play, @"Следующий трек", @"", 7);
    command(play, @"Предыдущий трек", @"", 8);
    NSMenu *window = submenu(bar, @"Окно");
    command(window, @"Показать плеер", @"", 0);
    command(window, @"Мини-плеер", @"", 10);
    command(window, @"Закрыть окно", @"w", 1);
    item(window, @"Свернуть", @selector(performMiniaturize:), @"m", nil);
    NSMenuItem *full = item(window, @"Полноэкранный режим", @selector(fullscreen:), @"f", delegate);
    full.keyEquivalentModifierMask = NSEventModifierFlagControl | NSEventModifierFlagCommand;
    NSApp.windowsMenu = window;
    applicationMenu = bar;
}

bool ym_start(YMCallback cb) {
    NSCAssert(NSThread.isMainThread, @"main thread");
    if (delegate || !extendWinitDelegate()) return false;
    callback = cb;
    previousMenu = NSApp.mainMenu;
    delegate = [YMApplicationDelegate new];
    buildMenu();
    ym_media_start(cb);
    screensObserver = [NSNotificationCenter.defaultCenter addObserverForName:NSApplicationDidChangeScreenParametersNotification
        object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) { sendCommand(15, 0); }];
    return true;
}
void ym_prepare_window(void) {
    NSCAssert(NSThread.isMainThread, @"main thread");
    if (!delegate) return;
    playerWindow = NSApp.keyWindow ?: NSApp.mainWindow ?: NSApp.windows.firstObject;
    if (playerWindow) {
        playerWindow.tabbingMode = NSWindowTabbingModeDisallowed;
        playerWindow.collectionBehavior |= NSWindowCollectionBehaviorFullScreenPrimary;
    }
    // Slint создаёт стандартное меню при создании первого окна; заменяем после этого.
    NSApp.mainMenu = applicationMenu;
}
void ym_activate(void) {
    [NSApp unhide:nil];
    [NSApp activateIgnoringOtherApps:YES];
    [playerWindow makeKeyAndOrderFront:nil];
}
void ym_stop(void) {
    NSCAssert(NSThread.isMainThread, @"main thread");
    callback = NULL;
    ym_media_stop();
    if (screensObserver) [NSNotificationCenter.defaultCenter removeObserver:screensObserver];
    screensObserver = nil;
    if (NSApp.delegate == savedApplicationDelegate && originalDelegateClass) object_setClass(savedApplicationDelegate, originalDelegateClass);
    savedApplicationDelegate = nil;
    originalDelegateClass = Nil;
    if (NSApp.mainMenu == applicationMenu) NSApp.mainMenu = previousMenu;
    applicationMenu = nil;
    delegate = nil;
    playerWindow = nil;
}

bool ym_light_theme(void) {
    NSString *match = [NSApp.effectiveAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]];
    return ![match isEqualToString:NSAppearanceNameDarkAqua];
}
bool ym_visible_area(double *left, double *top, double *right, double *bottom) {
    NSScreen *screen = playerWindow.screen;
    if (!screen) return false;
    NSRect f = screen.frame, v = screen.visibleFrame;
    *left = NSMinX(v) - NSMinX(f); *top = NSMaxY(f) - NSMaxY(v);
    *right = NSMaxX(v) - NSMinX(f); *bottom = NSMaxY(f) - NSMinY(v);
    return true;
}
void ym_ensure_visible(void) {
    if (!playerWindow || (playerWindow.styleMask & NSWindowStyleMaskFullScreen)) return;
    NSRect frame = playerWindow.frame;
    for (NSScreen *s in NSScreen.screens) {
        NSRect overlap = NSIntersectionRect(frame, s.visibleFrame);
        if (overlap.size.width >= 100 && overlap.size.height >= 80) return;
    }
    NSScreen *screen = playerWindow.screen ?: NSScreen.mainScreen;
    if (!screen) return;
    NSRect area = screen.visibleFrame;
    frame.size.width = MIN(frame.size.width, area.size.width);
    frame.size.height = MIN(frame.size.height, area.size.height);
    frame.origin.x = NSMidX(area) - frame.size.width / 2;
    frame.origin.y = NSMidY(area) - frame.size.height / 2;
    [playerWindow setFrame:frame display:YES];
}

void ym_menu_playback(bool has, bool playing) {
    mediaHasTrack = has;
    playItem.title = playing ? @"Пауза" : @"Воспроизвести";
}

void ym_finish_termination(void) {
    if (terminationPending) {
        terminationPending = false;
        [NSApp replyToApplicationShouldTerminate:YES];
    }
}

bool ym_is_fullscreen(void) { return (playerWindow.styleMask & NSWindowStyleMaskFullScreen) != 0; }
void ym_set_mini(bool mini) {
    miniMode = mini;
    NSWindowCollectionBehavior behavior = playerWindow.collectionBehavior;
    behavior &= ~(NSWindowCollectionBehaviorFullScreenPrimary | NSWindowCollectionBehaviorFullScreenNone);
    playerWindow.collectionBehavior = behavior | (mini ? NSWindowCollectionBehaviorFullScreenNone : NSWindowCollectionBehaviorFullScreenPrimary);
}
