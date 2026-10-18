import AppKit
import DzbanBridge

struct ResultTablePosition {
    let headers: [Column]
    let origin: NSPoint
    let widths: [CGFloat]
    let row: Int?
    let column: Int
    let rowNumberSelected: Bool
}
