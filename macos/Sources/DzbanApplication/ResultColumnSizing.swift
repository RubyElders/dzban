import AppKit

struct ResultColumnSizing {
    var preferred: CGFloat
    var assigned: CGFloat
    var manual = false
    var sampled = false

    static func fit(_ columns: [Self], available: CGFloat) -> [CGFloat] {
        var widths = columns.map { $0.manual ? $0.assigned : min($0.preferred, 360) }
        var spare = max(0, available - widths.reduce(0, +))
        for index in widths.indices where !columns[index].manual {
            let extra = min(max(0, columns[index].preferred - widths[index]), spare)
            widths[index] += extra
            spare -= extra
        }
        return widths
    }
}
