#import "LudionViewController.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#import <QuartzCore/CAMetalLayer.h>

#import "ludion.h"

// The view the engine presents into.
@interface LudionMetalView : UIView
@end

@implementation LudionMetalView

+ (Class)layerClass {
    return [CAMetalLayer class];
}

@end

@implementation LudionViewController {
    LudionEmbed *_game;
    BOOL _startAttempted;  // a failed start is logged once and never retried; a stop is final
    uint32_t _size[6];     // width, height, safe x, y, width, height, as last handed to the game
    CGFloat _scale;
    UIView *_controls;     // the game's touch controls, drawn; never hit-tested
    UIView *_resumeView;
    NSTimer *_endedTimer;
}

- (void)loadView {
    LudionMetalView *view = [[LudionMetalView alloc] init];
    view.multipleTouchEnabled = YES;
    self.view = view;
}

- (void)viewDidLoad {
    [super viewDidLoad];
    _controls = [[UIView alloc] init];
    _controls.userInteractionEnabled = NO;
    [self.view addSubview:_controls];

    _resumeView = [[UIView alloc] init];
    _resumeView.userInteractionEnabled = NO;
    _resumeView.backgroundColor = [UIColor colorWithWhite:0 alpha:0.6];
    _resumeView.hidden = YES;
    _resumeView.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    UILabel *hint = [[UILabel alloc] init];
    hint.text = @"Tap to resume";
    hint.textColor = UIColor.whiteColor;
    hint.font = [UIFont systemFontOfSize:28 weight:UIFontWeightSemibold];
    hint.textAlignment = NSTextAlignmentCenter;
    hint.frame = _resumeView.bounds;
    hint.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [_resumeView addSubview:hint];
    [self.view addSubview:_resumeView];

    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    [center addObserver:self
               selector:@selector(sceneSuspend:)
                   name:UISceneWillDeactivateNotification
                 object:nil];
    [center addObserver:self
               selector:@selector(sceneSuspend:)
                   name:UISceneDidEnterBackgroundNotification
                 object:nil];
    [center addObserver:self
               selector:@selector(sceneDidDisconnect:)
                   name:UISceneDidDisconnectNotification
                 object:nil];
}

- (BOOL)prefersStatusBarHidden {
    return YES;
}

- (BOOL)prefersHomeIndicatorAutoHidden {
    return YES;
}

- (UIRectEdge)preferredScreenEdgesDeferringSystemGestures {
    return UIRectEdgeAll;
}

- (UIInterfaceOrientationMask)supportedInterfaceOrientations {
    return UIInterfaceOrientationMaskLandscape;
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    CGFloat scale = self.view.window.screen.scale ?: UIScreen.mainScreen.scale;
    CGRect bounds = self.view.bounds;
    if (bounds.size.width <= 0 || bounds.size.height <= 0) {
        return;
    }
    _scale = scale;
    self.view.contentScaleFactor = scale;
    self.view.layer.contentsScale = scale;
    _controls.frame = bounds;
    _resumeView.frame = bounds;

    UIEdgeInsets insets = self.view.safeAreaInsets;
    CGRect safe = UIEdgeInsetsInsetRect(bounds, insets);
    uint32_t size[6] = {
        (uint32_t)lround(bounds.size.width * scale),
        (uint32_t)lround(bounds.size.height * scale),
        (uint32_t)lround(safe.origin.x * scale),
        (uint32_t)lround(safe.origin.y * scale),
        (uint32_t)lround(safe.size.width * scale),
        (uint32_t)lround(safe.size.height * scale),
    };
    if (_game == NULL) {
        if (!_startAttempted) {
            _startAttempted = YES;
            [self startWithSize:size];
        }
    } else if (memcmp(size, _size, sizeof size) != 0) {
        ludion_resize(_game, size[0], size[1], size[2], size[3], size[4], size[5]);
        [self rebuildControls];
    }
    memcpy(_size, size, sizeof size);
}

- (void)startWithSize:(const uint32_t *)size {
    NSString *content = [NSBundle.mainBundle.resourcePath stringByAppendingPathComponent:@"content"];
    NSURL *support = [NSFileManager.defaultManager URLForDirectory:NSApplicationSupportDirectory
                                                          inDomain:NSUserDomainMask
                                                 appropriateForURL:nil
                                                            create:YES
                                                             error:nil];
    LudionStartParams params = {
        .content_root = content.fileSystemRepresentation,
        .save_root = support.path.fileSystemRepresentation,
        .surface_kind = LUDION_SURFACE_UIKIT,
        .view = (__bridge void *)self.view,
        .width = size[0],
        .height = size[1],
        .paused = false,
        .safe_x = size[2],
        .safe_y = size[3],
        .safe_width = size[4],
        .safe_height = size[5],
        .quality = "{{quality}}",
    };
    _game = ludion_game_start(&params);
    if (_game == NULL) {
        NSLog(@"ludion_game_start failed; the diagnostic is on stderr");
        return;
    }
    [self setNeedsUpdateOfHomeIndicatorAutoHidden];
    [self setNeedsUpdateOfScreenEdgesDeferringSystemGestures];
    [self rebuildControls];
    // An argv mode, a wire engine.shutdown, a failed boot or a fatal render fault ends the engine,
    // not the app: exit so whatever launched the app sees it finish.
    _endedTimer = [NSTimer scheduledTimerWithTimeInterval:0.25
                                                   target:self
                                                 selector:@selector(checkEnded:)
                                                 userInfo:nil
                                                  repeats:YES];
}

- (void)checkEnded:(NSTimer *)timer {
    if (_game != NULL && ludion_ended(_game)) {
        [timer invalidate];
        _endedTimer = nil;
        LudionEmbed *game = _game;
        _game = NULL;
        exit(ludion_stop(game));
    }
}

// Redraws the game's touch controls at the rects it reports; they move with every resize.
- (void)rebuildControls {
    for (UIView *view in [_controls.subviews copy]) {
        [view removeFromSuperview];
    }
    if (_game == NULL || _scale <= 0) {
        return;
    }
    UIColor *line = [UIColor colorWithWhite:1 alpha:0.6];
    uint32_t count = ludion_touch_control_count(_game);
    for (uint32_t i = 0; i < count; i++) {
        LudionTouchControl control;
        if (!ludion_touch_control(_game, i, &control)) {
            continue;
        }
        UIView *view = [[UIView alloc] initWithFrame:CGRectMake(control.x / _scale, control.y / _scale,
                                                                control.width / _scale,
                                                                control.height / _scale)];
        view.layer.borderColor = line.CGColor;
        view.layer.borderWidth = 2;
        if (control.kind == 2) {
            view.layer.cornerRadius = 12;
            view.backgroundColor = [UIColor colorWithWhite:1 alpha:0.15];
            UILabel *label = [[UILabel alloc] initWithFrame:view.bounds];
            label.text = control.label ? [NSString stringWithUTF8String:control.label] : @"";
            label.textColor = UIColor.whiteColor;
            label.font = [UIFont systemFontOfSize:22 weight:UIFontWeightBold];
            label.textAlignment = NSTextAlignmentCenter;
            [view addSubview:label];
        } else {
            view.layer.cornerRadius = MIN(view.bounds.size.width, view.bounds.size.height) / 2;
        }
        [_controls addSubview:view];
    }
}

- (void)forwardTouches:(NSSet<UITouch *> *)touches phase:(uint32_t)phase {
    if (_game == NULL) {
        return;
    }
    for (UITouch *touch in touches) {
        CGPoint at = [touch locationInView:self.view];
        int32_t result = ludion_touch(_game, (uint64_t)(uintptr_t)(__bridge void *)touch, phase,
                                      at.x * _scale, at.y * _scale);
        if (result == LUDION_TOUCH_RESUMED) {
            _resumeView.hidden = YES;
        }
    }
}

- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:LUDION_TOUCH_BEGAN];
}

- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:LUDION_TOUCH_MOVED];
}

- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:LUDION_TOUCH_ENDED];
}

- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:LUDION_TOUCH_CANCELLED];
}

// Deactivation (Control Center, a call banner, the app switcher) comes before backgrounding, and
// suspending twice is harmless. Returning does nothing: the engine stays paused behind the view
// until the next touch.
- (void)sceneSuspend:(NSNotification *)note {
    if (_game != NULL && ludion_suspend(_game)) {
        _resumeView.hidden = NO;
    }
}

- (void)sceneDidDisconnect:(NSNotification *)note {
    [_endedTimer invalidate];
    _endedTimer = nil;
    if (_game != NULL) {
        LudionEmbed *game = _game;
        _game = NULL;
        ludion_stop(game);
    }
}

@end
