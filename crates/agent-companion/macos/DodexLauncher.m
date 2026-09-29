#import <Foundation/Foundation.h>
#include <unistd.h>

int main(void) {
    @autoreleasepool {
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *name = [bundle objectForInfoDictionaryKey:@"DodexNativeExecutable"];
        NSDictionary *environment = [bundle objectForInfoDictionaryKey:@"LSEnvironment"];
        NSString *data = [environment isKindOfClass:NSDictionary.class]
            ? environment[@"CODEX_ELECTRON_USER_DATA_PATH"] : nil;
        if (![name isKindOfClass:NSString.class] || name.length == 0 ||
            ![name.lastPathComponent isEqualToString:name] ||
            [name isEqualToString:@"."] || [name isEqualToString:@".."] ||
            [name isEqualToString:@"DodexLauncher"] ||
            ![data isKindOfClass:NSString.class] || !data.isAbsolutePath) {
            fputs("Dodex: invalid instance configuration\n", stderr);
            return 78;
        }
        NSString *native = [[bundle.bundlePath stringByAppendingPathComponent:@"Contents/MacOS"]
            stringByAppendingPathComponent:name];
        NSString *profile = [@"--user-data-dir=" stringByAppendingString:data];
        // execv preserves the PID and the public App's Dock identity. Launch
        // arguments cannot override the profile; task navigation uses Apple events.
        char *const arguments[] = {
            (char *)native.fileSystemRepresentation, (char *)profile.UTF8String, NULL
        };
        execv(arguments[0], arguments);
        perror("Dodex: cannot start native executable");
        return 127;
    }
}
