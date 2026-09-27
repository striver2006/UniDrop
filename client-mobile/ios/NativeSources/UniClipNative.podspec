Pod::Spec.new do |s|
  s.name             = 'UniClipNative'
  s.version          = '0.4.2'
  s.summary          = 'UniClip Rust core (unidrop-mobile-native) static library'
  s.description      = '平台无关核心（协议 / ARQ / E2EE / TLS / 存储）的 iOS 静态库产物'
  s.homepage         = 'https://github.com/unidrop/uniclip'
  s.license          = { :type => 'Apache-2.0' }
  s.author           = { 'UniDrop Team' => '' }
  s.source           = { :path => '.' }
  s.ios.deployment_target = '15.0'
  s.vendored_libraries = 'lib/libunidrop_mobile.a'
  s.public_header_files = 'include/unidrop_mobile.h'
  s.source_files = 'include/unidrop_mobile.h'
  s.preserve_paths = 'lib/libunidrop_mobile.a'
  s.pod_target_xcconfig = {
    'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => '',
  }
end
