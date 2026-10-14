// !$*UTF8*$!
{
	archiveVersion = 1;
	classes = {
	};
	objectVersion = 56;
	objects = {

/* Begin PBXBuildFile section */
		A00000000000000000000020 /* main.m in Sources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000010 /* main.m */; };
		A00000000000000000000021 /* LudionViewController.m in Sources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000011 /* LudionViewController.m */; };
		A00000000000000000000022 /* content in Resources */ = {isa = PBXBuildFile; fileRef = A00000000000000000000015 /* content */; };
/* End PBXBuildFile section */

/* Begin PBXFileReference section */
		A00000000000000000000009 /* {{name}}.app */ = {isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = "{{name}}.app"; sourceTree = BUILT_PRODUCTS_DIR; };
		A00000000000000000000010 /* main.m */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = main.m; sourceTree = "<group>"; };
		A00000000000000000000011 /* LudionViewController.m */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = LudionViewController.m; sourceTree = "<group>"; };
		A00000000000000000000012 /* LudionViewController.h */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = LudionViewController.h; sourceTree = "<group>"; };
		A00000000000000000000013 /* ludion.h */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = ludion.h; sourceTree = "<group>"; };
		A00000000000000000000014 /* Info.plist */ = {isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; };
		A00000000000000000000015 /* content */ = {isa = PBXFileReference; lastKnownFileType = folder; path = content; sourceTree = SOURCE_ROOT; };
/* End PBXFileReference section */

/* Begin PBXGroup section */
		A00000000000000000000002 = {
			isa = PBXGroup;
			children = (
				A00000000000000000000003 /* Host */,
				A00000000000000000000015 /* content */,
				A00000000000000000000004 /* Products */,
			);
			sourceTree = "<group>";
		};
		A00000000000000000000003 /* Host */ = {
			isa = PBXGroup;
			children = (
				A00000000000000000000010 /* main.m */,
				A00000000000000000000011 /* LudionViewController.m */,
				A00000000000000000000012 /* LudionViewController.h */,
				A00000000000000000000013 /* ludion.h */,
				A00000000000000000000014 /* Info.plist */,
			);
			path = Host;
			sourceTree = "<group>";
		};
		A00000000000000000000004 /* Products */ = {
			isa = PBXGroup;
			children = (
				A00000000000000000000009 /* {{name}}.app */,
			);
			name = Products;
			sourceTree = "<group>";
		};
/* End PBXGroup section */

/* Begin PBXNativeTarget section */
		A00000000000000000000005 /* {{name}} */ = {
			isa = PBXNativeTarget;
			buildConfigurationList = A00000000000000000000031 /* Build configuration list for PBXNativeTarget "{{name}}" */;
			buildPhases = (
				A00000000000000000000006 /* Sources */,
				A00000000000000000000007 /* Resources */,
			);
			buildRules = (
			);
			dependencies = (
			);
			name = "{{name}}";
			productName = "{{name}}";
			productReference = A00000000000000000000009 /* {{name}}.app */;
			productType = "com.apple.product-type.application";
		};
/* End PBXNativeTarget section */

/* Begin PBXProject section */
		A00000000000000000000001 /* Project object */ = {
			isa = PBXProject;
			attributes = {
				LastUpgradeCheck = 1500;
			};
			buildConfigurationList = A00000000000000000000030 /* Build configuration list for PBXProject "{{name}}" */;
			compatibilityVersion = "Xcode 14.0";
			developmentRegion = en;
			hasScannedForEncodings = 0;
			knownRegions = (
				en,
				Base,
			);
			mainGroup = A00000000000000000000002;
			productRefGroup = A00000000000000000000004 /* Products */;
			projectDirPath = "";
			projectRoot = "";
			targets = (
				A00000000000000000000005 /* {{name}} */,
			);
		};
/* End PBXProject section */

/* Begin PBXResourcesBuildPhase section */
		A00000000000000000000007 /* Resources */ = {
			isa = PBXResourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				A00000000000000000000022 /* content in Resources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		};
/* End PBXResourcesBuildPhase section */

/* Begin PBXSourcesBuildPhase section */
		A00000000000000000000006 /* Sources */ = {
			isa = PBXSourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				A00000000000000000000020 /* main.m in Sources */,
				A00000000000000000000021 /* LudionViewController.m in Sources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		};
/* End PBXSourcesBuildPhase section */

/* Begin XCBuildConfiguration section */
		A00000000000000000000032 /* Debug */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
				CLANG_ENABLE_OBJC_ARC = YES;
				CLANG_ENABLE_MODULES = YES;
				GCC_OPTIMIZATION_LEVEL = 0;
				GCC_PREPROCESSOR_DEFINITIONS = (
					"DEBUG=1",
					"$(inherited)",
				);
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				ONLY_ACTIVE_ARCH = YES;
				SDKROOT = iphoneos;
			};
			name = Debug;
		};
		A00000000000000000000033 /* Release */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
				CLANG_ENABLE_OBJC_ARC = YES;
				CLANG_ENABLE_MODULES = YES;
				IPHONEOS_DEPLOYMENT_TARGET = 16.0;
				SDKROOT = iphoneos;
				VALIDATE_PRODUCT = YES;
			};
			name = Release;
		};
		A00000000000000000000034 /* Debug */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
				ARCHS = arm64;
				CODE_SIGN_STYLE = Automatic;
				GENERATE_INFOPLIST_FILE = NO;
				HEADER_SEARCH_PATHS = "$(PROJECT_DIR)/Host";
				INFOPLIST_FILE = Host/Info.plist;
				OTHER_LDFLAGS = (
					"$(PROJECT_DIR)/Ludion/libgame.a",
					"-framework",
					UIKit,
					"-framework",
					Metal,
					"-framework",
					QuartzCore,
					"-framework",
					CoreGraphics,
					"-framework",
					Foundation,
					"-framework",
					CoreFoundation,
					"-framework",
					AudioToolbox,
					"-framework",
					CoreAudio,
					"-liconv",
				);
				PRODUCT_BUNDLE_IDENTIFIER = "{{bundle_id}}";
				PRODUCT_NAME = "{{name}}";
				SUPPORTED_PLATFORMS = "iphoneos iphonesimulator";
				TARGETED_DEVICE_FAMILY = "1,2";
			};
			name = Debug;
		};
		A00000000000000000000035 /* Release */ = {
			isa = XCBuildConfiguration;
			buildSettings = {
				ARCHS = arm64;
				CODE_SIGN_STYLE = Automatic;
				GENERATE_INFOPLIST_FILE = NO;
				HEADER_SEARCH_PATHS = "$(PROJECT_DIR)/Host";
				INFOPLIST_FILE = Host/Info.plist;
				OTHER_LDFLAGS = (
					"$(PROJECT_DIR)/Ludion/libgame.a",
					"-framework",
					UIKit,
					"-framework",
					Metal,
					"-framework",
					QuartzCore,
					"-framework",
					CoreGraphics,
					"-framework",
					Foundation,
					"-framework",
					CoreFoundation,
					"-framework",
					AudioToolbox,
					"-framework",
					CoreAudio,
					"-liconv",
				);
				PRODUCT_BUNDLE_IDENTIFIER = "{{bundle_id}}";
				PRODUCT_NAME = "{{name}}";
				SUPPORTED_PLATFORMS = "iphoneos iphonesimulator";
				TARGETED_DEVICE_FAMILY = "1,2";
			};
			name = Release;
		};
/* End XCBuildConfiguration section */

/* Begin XCConfigurationList section */
		A00000000000000000000030 /* Build configuration list for PBXProject "{{name}}" */ = {
			isa = XCConfigurationList;
			buildConfigurations = (
				A00000000000000000000032 /* Debug */,
				A00000000000000000000033 /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		};
		A00000000000000000000031 /* Build configuration list for PBXNativeTarget "{{name}}" */ = {
			isa = XCConfigurationList;
			buildConfigurations = (
				A00000000000000000000034 /* Debug */,
				A00000000000000000000035 /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		};
/* End XCConfigurationList section */
	};
	rootObject = A00000000000000000000001 /* Project object */;
}
