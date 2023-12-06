// Copyright © 2023 David Caldwell <david@porkrind.org> -*- js-indent-level: 2; mode: js; -*-

module.exports = {
  packagerConfig: {
    icon: 'assets/floppy',
    osxUniversal: {},
    platform: ['darwin', 'win32'],
    osxSign: {
      identity: process.env.CODESIGN_IDENTITY,
      keychain: process.env.CODESIGN_KEYCHAIN,
    },
    appCategoryType: "public.app-category.utilities",
    appCopyright: "Copyright © 2023 David Caldwell <david@porkrind.org>",
    extendInfo: { /* extra Info.plist junk */ },
  },
  makers: [
    {
      name: "@electron-forge/maker-squirrel",
      config: {
        name: "electron_quick_start"
      }
    },
    {
      name: "@electron-forge/maker-zip",
      platforms: [
        "darwin"
      ]
    },
    {
      name: "@electron-forge/maker-dmg",
      platforms: [
        "darwin"
      ]
    },
    // {
    //   name: "@electron-forge/maker-deb",
    //   config: {}
    // },
    // {
    //   name: "@electron-forge/maker-rpm",
    //   config: {}
    // }
  ]
}
