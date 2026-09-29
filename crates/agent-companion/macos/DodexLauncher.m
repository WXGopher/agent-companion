#import <Foundation/Foundation.h>
#include <stdlib.h>
#include <unistd.h>

int main(void) {
    @autoreleasepool {
        NSBundle *bundle = NSBundle.mainBundle;
        NSString *app = [bundle.bundlePath copy];
        NSString *updater = [bundle objectForInfoDictionaryKey:@"DodexUpdaterExecutable"];
        if ([updater isKindOfClass:NSString.class] && updater.isAbsolutePath &&
            [NSFileManager.defaultManager isExecutableFileAtPath:updater]) {
            NSTask *task = [[NSTask alloc] init];
            task.executableURL = [NSURL fileURLWithPath:updater];
            task.arguments = @[@"dodex-app", @"--sync-on-launch", app];
            task.standardInput = NSFileHandle.fileHandleWithNullDevice;
            task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
            task.standardError = NSFileHandle.fileHandleWithNullDevice;
            if ([task launchAndReturnError:NULL]) {
                [task waitUntilExit];
                if (task.terminationReason != NSTaskTerminationReasonExit ||
                    task.terminationStatus != 0) {
                    fputs("Dodex: startup sync failed; using current app\n", stderr);
                }
            } else {
                fputs("Dodex: startup sync unavailable; using current app\n", stderr);
            }
        } else {
            fputs("Dodex: startup sync unavailable; using current app\n", stderr);
        }
        // The updater can replace this bundle while we wait. Read its public
        // path again without NSBundle's cached metadata or resolved location.
        NSDictionary *info = [NSDictionary dictionaryWithContentsOfURL:
            [NSURL fileURLWithPath:[app stringByAppendingPathComponent:@"Contents/Info.plist"]]
            error:NULL];
        NSString *name = info[@"DodexNativeExecutable"];
        NSDictionary *environment = info[@"LSEnvironment"];
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
        for (id key in environment) {
            id value = environment[key];
            if ([key isKindOfClass:NSString.class] && [value isKindOfClass:NSString.class]) {
                setenv([key UTF8String], [value UTF8String], 1);
            }
        }
        NSString *native = [[app stringByAppendingPathComponent:@"Contents/MacOS"]
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
