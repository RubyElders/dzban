import AppKit
import DzbanBridge

struct ResultTablePosition {
    let headers: [Column]
    let origin: NSPoint
    let widths: [CGFloat]
    let row: Int?
    let column: Int
    var sizing: [ResultColumnSizing]? = nil
    let rowNumberSelected: Bool
}
