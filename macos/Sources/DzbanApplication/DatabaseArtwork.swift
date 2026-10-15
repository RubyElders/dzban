import SwiftUI

struct DatabaseArtwork: View {
    let backend: String
    var body: some View {
        Image(backend == "clickhouse" ? "dzban-clickhouse" : "dzban-postgresql", bundle: .module)
            .resizable().renderingMode(.original).scaledToFit().frame(width: 18, height: 18)
            .accessibilityLabel(backend == "clickhouse" ? "ClickHouse" : "PostgreSQL")
    }
}

struct FooterArtwork: View {
    let name: String
    var body: some View {
        Image(name, bundle: .module).resizable().renderingMode(.template).scaledToFit().frame(width: 16, height: 16)
            .accessibilityHidden(true)
    }
}
