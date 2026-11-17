#import <Cocoa/Cocoa.h>
#import <MediaPlayer/MediaPlayer.h>
#include <stdint.h>

typedef void (*YMCallback)(int32_t, double);
static YMCallback callback;
static NSMutableArray<NSArray *> *targets;
static NSURLSessionDataTask *artworkTask;
static NSURLSession *artworkSession;
static NSString *trackID;
static NSString *coverURL;
static NSUInteger generation;
static bool hasTrack;
extern void ym_menu_playback(bool has, bool playing);

static void registerCommand(MPRemoteCommand *command, int32_t code) {
    NSUInteger session = generation;
    id target = [command addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *event) {
        double value = [event isKindOfClass:MPChangePlaybackPositionCommandEvent.class]
            ? ((MPChangePlaybackPositionCommandEvent *)event).positionTime : 0;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (session == generation && callback && hasTrack) callback(code, value);
        });
        return MPRemoteCommandHandlerStatusSuccess;
    }];
    command.enabled = NO;
    [targets addObject:@[command, target]];
}
void ym_media_start(YMCallback cb) {
    callback = cb;
    generation++;
    targets = [NSMutableArray new];
    MPRemoteCommandCenter *center = MPRemoteCommandCenter.sharedCommandCenter;
    registerCommand(center.playCommand, 4);
    registerCommand(center.pauseCommand, 5);
    registerCommand(center.stopCommand, 5);
    registerCommand(center.togglePlayPauseCommand, 6);
    registerCommand(center.nextTrackCommand, 7);
    registerCommand(center.previousTrackCommand, 8);
    registerCommand(center.changePlaybackPositionCommand, 9);
    NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    config.HTTPShouldSetCookies = NO;
    config.timeoutIntervalForRequest = 10;
    artworkSession = [NSURLSession sessionWithConfiguration:config];
}
void ym_media_stop(void) {
    callback = NULL;
    generation++;
    for (NSArray *entry in targets) {
        MPRemoteCommand *command = entry[0];
        [command removeTarget:entry[1]];
        command.enabled = NO;
    }
    targets = nil;
    [artworkTask cancel]; artworkTask = nil;
    [artworkSession invalidateAndCancel]; artworkSession = nil;
    trackID = nil; coverURL = nil; hasTrack = false;
    MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo = nil;
    MPNowPlayingInfoCenter.defaultCenter.playbackState = MPNowPlayingPlaybackStateStopped;
}
void ym_media_update(const uint8_t *bytes, size_t len) {
    NSCAssert(NSThread.isMainThread, @"main thread");
    if (!callback) return;
    NSData *data = [NSData dataWithBytes:bytes length:len];
    NSDictionary *state = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![state isKindOfClass:NSDictionary.class]) return;
    NSString *newID = state[@"id"];
    BOOL playing = [state[@"playing"] boolValue];
    hasTrack = newID.length > 0;
    ym_menu_playback(hasTrack, playing);
    for (NSArray *entry in targets) ((MPRemoteCommand *)entry[0]).enabled = hasTrack;
    if (!hasTrack) {
        [artworkTask cancel]; artworkTask = nil;
        trackID = nil; coverURL = nil;
        MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo = nil;
        MPNowPlayingInfoCenter.defaultCenter.playbackState = MPNowPlayingPlaybackStateStopped;
        return;
    }
    NSString *newCover = state[@"cover"] ?: @"";
    BOOL changed = ![newID isEqualToString:trackID] || ![newCover isEqualToString:coverURL];
    NSMutableDictionary *info = changed ? [NSMutableDictionary new]
        : [MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo mutableCopy];
    if (!info) info = [NSMutableDictionary new];
    info[MPMediaItemPropertyTitle] = state[@"title"] ?: @"";
    info[MPMediaItemPropertyArtist] = state[@"artist"] ?: @"";
    info[MPMediaItemPropertyAlbumTitle] = state[@"album"] ?: @"";
    info[MPMediaItemPropertyPlaybackDuration] = state[@"duration"] ?: @0;
    info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = state[@"position"] ?: @0;
    info[MPNowPlayingInfoPropertyPlaybackRate] = playing ? @1 : @0;
    info[MPNowPlayingInfoPropertyDefaultPlaybackRate] = @1;
    info[MPNowPlayingInfoPropertyMediaType] = @(MPNowPlayingInfoMediaTypeAudio);
    MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo = info;
    MPNowPlayingInfoCenter.defaultCenter.playbackState = playing
        ? MPNowPlayingPlaybackStatePlaying : MPNowPlayingPlaybackStatePaused;
    if (!changed) return;
    trackID = [newID copy];
    coverURL = [newCover copy];
    [artworkTask cancel];
    NSURL *url = [NSURL URLWithString:state[@"cover"] ?: @""];
    if (![url.scheme isEqualToString:@"https"]) return;
    NSUInteger session = generation;
    NSString *expectedID = trackID;
    NSString *expectedCover = coverURL;
    artworkTask = [artworkSession dataTaskWithURL:url completionHandler:^(NSData *imageData, NSURLResponse *response, NSError *error) {
        if (error || !imageData || imageData.length > 10 * 1024 * 1024) return;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (session != generation || ![trackID isEqualToString:expectedID] || ![coverURL isEqualToString:expectedCover]) return;
            NSImage *image = [[NSImage alloc] initWithData:imageData];
            if (!image) return;
            MPMediaItemArtwork *artwork = [[MPMediaItemArtwork alloc] initWithBoundsSize:image.size
                requestHandler:^NSImage *(CGSize size) { return image; }];
            NSMutableDictionary *latest = [MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo mutableCopy];
            if (!latest) return;
            latest[MPMediaItemPropertyArtwork] = artwork;
            MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo = latest;
        });
    }];
    [artworkTask resume];
}
