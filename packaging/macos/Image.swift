import AppKit

// Copy Rust's RGBA allocation into Core Graphics before releasing it.
func thumbnailImage(_ url: URL, size: UInt32) throws -> CGImage {
    var w: UInt32 = 0, h: UInt32 = 0
    var pixels: UnsafeMutablePointer<UInt8>?
    var count = 0
    let status = url.path.withCString {
        tmv_thumbnail_rgba($0, min(max(size, 1), 2048), &w, &h, &pixels, &count)
    }
    guard status == 0, let pixels else {
        throw NSError(domain: "com.leehow.tifmrcview.thumbnail", code: Int(status))
    }
    defer { tmv_free(pixels, count) }
    let data = Data(bytes: pixels, count: count)
    guard let provider = CGDataProvider(data: data as CFData),
          let image = CGImage(width: Int(w), height: Int(h), bitsPerComponent: 8,
            bitsPerPixel: 32, bytesPerRow: Int(w) * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent) else {
        throw NSError(domain: "com.leehow.tifmrcview.thumbnail", code: 3)
    }
    return image
}
