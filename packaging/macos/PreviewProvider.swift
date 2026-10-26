import QuickLookUI
import UniformTypeIdentifiers
import AppKit

@objc(PreviewProvider)
final class PreviewProvider: QLPreviewProvider, QLPreviewingController {
    func providePreview(for request: QLFilePreviewRequest,
        completionHandler handler: @escaping (QLPreviewReply?, Error?) -> Void) {
        do {
            let image = try thumbnailImage(request.fileURL, size: 1600)
            let bitmap = NSBitmapImageRep(cgImage: image)
            guard let png = bitmap.representation(using: .png, properties: [:]) else {
                throw NSError(domain: "com.leehow.tifmrcview.preview", code: 1)
            }
            handler(QLPreviewReply(dataOfContentType: .png,
                contentSize: CGSize(width: image.width, height: image.height),
                createDataUsing: { _ in png }), nil)
        } catch { handler(nil, error) }
    }
}
