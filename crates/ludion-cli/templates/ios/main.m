// The app's entry: UIKit boots a window scene whose one view hosts the game.
#import <UIKit/UIKit.h>

#import "LudionViewController.h"

@interface LudionAppDelegate : UIResponder <UIApplicationDelegate>
@end

@implementation LudionAppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary<UIApplicationLaunchOptionsKey, id> *)launchOptions {
    return YES;
}

@end

@interface LudionSceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation LudionSceneDelegate

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)connectionOptions {
    if (![scene isKindOfClass:[UIWindowScene class]]) {
        return;
    }
    self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
    self.window.rootViewController = [[LudionViewController alloc] init];
    [self.window makeKeyAndVisible];
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, @"LudionAppDelegate");
    }
}
