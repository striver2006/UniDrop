/// iOS 机型标识 → 营销名映射（utsname.machine，形如 "iPhone18,1"）。
///
/// 用途：iOS 16 起 Apple 把 `UIDevice.name` 对第三方 App 脱敏成通用型号名
/// （"iPhone"/"iPad"），真实设备名不再可得；设置页的「设备名」覆盖是主路径，
/// 这里只给未设置时的兜底一个有区分度的名字（"iPhone 16 Pro" 而非 "iPhone"）。
///
/// 数据来源：Apple 机型标识社区总表（adamawolf gist）等，收录至 2025 年
/// 在售机型。**新机型发布后需要补条目**——识别不了返回 null，调用方回落
/// 系统通用名，不会出错，只是不够精确。
///
/// 保持 Dart 2.19 兼容（鸿蒙 Flutter 基座 3.7/Dart 2.19，三端一套代码）。

library;

/// iOS 16+ 对无受管 entitlement 的 App 返回的通用型号名（iPadOS 同理）。
/// iOS 15 及以下、以及模拟器返回的不是这些值（模拟器如 "iPhone Simulator"）。
bool isGenericIosName(String name) =>
    name == 'iPhone' || name == 'iPad' || name == 'iPod touch';

/// machine 标识（utsname.machine）→ 机型营销名。未知标识返回 null。
String? iosMarketingName(String machine) => _iosModels[machine];

const _iosModels = <String, String>{
  // ---- iPhone（iPhone 8 / X 起；更早的机型跑不动现役 Flutter 最低系统）----
  'iPhone10,1': 'iPhone 8',
  'iPhone10,4': 'iPhone 8',
  'iPhone10,2': 'iPhone 8 Plus',
  'iPhone10,5': 'iPhone 8 Plus',
  'iPhone10,3': 'iPhone X',
  'iPhone10,6': 'iPhone X',
  'iPhone11,2': 'iPhone XS',
  'iPhone11,4': 'iPhone XS Max',
  'iPhone11,6': 'iPhone XS Max',
  'iPhone11,8': 'iPhone XR',
  'iPhone12,1': 'iPhone 11',
  'iPhone12,3': 'iPhone 11 Pro',
  'iPhone12,5': 'iPhone 11 Pro Max',
  'iPhone12,8': 'iPhone SE (2nd gen)',
  'iPhone13,1': 'iPhone 12 mini',
  'iPhone13,2': 'iPhone 12',
  'iPhone13,3': 'iPhone 12 Pro',
  'iPhone13,4': 'iPhone 12 Pro Max',
  'iPhone14,2': 'iPhone 13 Pro',
  'iPhone14,3': 'iPhone 13 Pro Max',
  'iPhone14,4': 'iPhone 13 mini',
  'iPhone14,5': 'iPhone 13',
  'iPhone14,6': 'iPhone SE (3rd gen)',
  'iPhone14,7': 'iPhone 14',
  'iPhone14,8': 'iPhone 14 Plus',
  'iPhone15,2': 'iPhone 14 Pro',
  'iPhone15,3': 'iPhone 14 Pro Max',
  'iPhone15,4': 'iPhone 15',
  'iPhone15,5': 'iPhone 15 Plus',
  'iPhone16,1': 'iPhone 15 Pro',
  'iPhone16,2': 'iPhone 15 Pro Max',
  'iPhone17,1': 'iPhone 16 Pro',
  'iPhone17,2': 'iPhone 16 Pro Max',
  'iPhone17,3': 'iPhone 16',
  'iPhone17,4': 'iPhone 16 Plus',
  'iPhone17,5': 'iPhone 16e',
  'iPhone18,1': 'iPhone 17 Pro',
  'iPhone18,2': 'iPhone 17 Pro Max',
  'iPhone18,3': 'iPhone 17',
  'iPhone18,4': 'iPhone Air',
  'iPhone18,5': 'iPhone 17e',

  // ---- iPad（2019 起的主流在售款；Pro 2018 等更早款不再收录）----
  'iPad11,1': 'iPad mini (5th gen)',
  'iPad11,2': 'iPad mini (5th gen)',
  'iPad11,3': 'iPad Air (3rd gen)',
  'iPad11,4': 'iPad Air (3rd gen)',
  'iPad12,1': 'iPad (9th gen)',
  'iPad12,2': 'iPad (9th gen)',
  'iPad13,1': 'iPad Air (4th gen)',
  'iPad13,2': 'iPad Air (4th gen)',
  'iPad13,4': 'iPad Pro 11-inch (M1)',
  'iPad13,5': 'iPad Pro 11-inch (M1)',
  'iPad13,6': 'iPad Pro 11-inch (M1)',
  'iPad13,7': 'iPad Pro 11-inch (M1)',
  'iPad13,8': 'iPad Pro 12.9-inch (M1)',
  'iPad13,9': 'iPad Pro 12.9-inch (M1)',
  'iPad13,10': 'iPad Pro 12.9-inch (M1)',
  'iPad13,11': 'iPad Pro 12.9-inch (M1)',
  'iPad13,16': 'iPad Air (5th gen)',
  'iPad13,17': 'iPad Air (5th gen)',
  'iPad14,1': 'iPad mini (6th gen)',
  'iPad14,2': 'iPad mini (6th gen)',
  'iPad14,3': 'iPad Pro 11-inch (M2)',
  'iPad14,4': 'iPad Pro 11-inch (M2)',
  'iPad14,5': 'iPad Pro 12.9-inch (M2)',
  'iPad14,6': 'iPad Pro 12.9-inch (M2)',
  'iPad14,7': 'iPad (10th gen)',
  'iPad14,8': 'iPad (10th gen)',
  'iPad14,9': 'iPad Air 11-inch (M2)',
  'iPad14,10': 'iPad Air 11-inch (M2)',
  'iPad14,11': 'iPad Air 13-inch (M2)',
  'iPad14,12': 'iPad Air 13-inch (M2)',
  'iPad15,3': 'iPad Pro 11-inch (M4)',
  'iPad15,4': 'iPad Pro 11-inch (M4)',
  'iPad15,5': 'iPad Pro 13-inch (M4)',
  'iPad15,6': 'iPad Pro 13-inch (M4)',
  'iPad15,7': 'iPad (11th gen)',
  'iPad15,8': 'iPad (11th gen)',
  'iPad16,1': 'iPad mini (7th gen)',
  'iPad16,2': 'iPad mini (7th gen)',
};
