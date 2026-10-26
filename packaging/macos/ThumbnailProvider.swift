import QuickLookThumbnailing
import AppKit

@objc(ThumbnailProvider)
final class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(for request: QLFileThumbnailRequest,
        _ handler: @escaping (QLThumbnailReply?, Error?) -> Void) {
        do {
            let limit = min(2048, max(1, max(request.maximumSize.width, request.maximumSize.height) * request.scale))
            let image = try thumbnailImage(request.fileURL, size: UInt32(limit))
            let factor = min(1 / request.scale, min(
                request.maximumSize.width / CGFloat(image.width),
                request.maximumSize.height / CGFloat(image.height)))
            let size = CGSize(width: CGFloat(image.width) * factor, height: CGFloat(image.height) * factor)
            handler(QLThumbnailReply(contextSize: size, currentContextDrawing: {
                guard let context = NSGraphicsContext.current?.cgContext else { return false }
                context.draw(image, in: CGRect(origin: .zero, size: size))
                return true
            }), nil)
        } catch { handler(nil, error) }
    }
}
