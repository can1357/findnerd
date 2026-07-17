use clap::Args;

macro_rules! category_flags {
    ($(($include:ident, $exclude:ident, $slug:literal, $name:literal)),+ $(,)?) => {
        /// Direct include/exclude switches for every embedded glyph collection.
        #[derive(Args, Debug, Default)]
        pub struct CategoryFlags {
            $(
                #[arg(
                    long = $slug,
                    help = concat!("Include ", $name, "; any include switch makes categories opt-in."),
                    help_heading = "Category switches"
                )]
                $include: bool,

                #[arg(
                    long = concat!("no-", $slug),
                    help = concat!("Exclude ", $name, "; exclusions take precedence."),
                    help_heading = "Category switches"
                )]
                $exclude: bool,
            )+
        }

        impl CategoryFlags {
            /// Returns category slugs explicitly opted into by positive switches.
            pub(crate) fn included(&self) -> Vec<String> {
                let mut selected = Vec::new();
                $(
                    if self.$include {
                        selected.push($slug.to_owned());
                    }
                )+
                selected
            }

            /// Returns category slugs opted out of by negative switches.
            pub(crate) fn excluded(&self) -> Vec<String> {
                let mut excluded = Vec::new();
                $(
                    if self.$exclude {
                        excluded.push($slug.to_owned());
                    }
                )+
                excluded
            }
        }
    };
}

category_flags!(
	(cod, no_cod, "cod", "VS Code Codicons"),
	(custom, no_custom, "custom", "Nerd Fonts Custom"),
	(dev, no_dev, "dev", "Devicons"),
	(extra, no_extra, "extra", "Font Awesome Extension extras"),
	(fa, no_fa, "fa", "Font Awesome"),
	(fae, no_fae, "fae", "Font Awesome Extension"),
	(iec, no_iec, "iec", "IEC Power Symbols"),
	(indent, no_indent, "indent", "Indentation symbols"),
	(indentation, no_indentation, "indentation", "legacy indentation symbols"),
	(linux, no_linux, "linux", "Font Logos"),
	(md, no_md, "md", "Material Design Icons"),
	(oct, no_oct, "oct", "GitHub Octicons"),
	(pl, no_pl, "pl", "Powerline"),
	(ple, no_ple, "ple", "Powerline Extra"),
	(pom, no_pom, "pom", "Pomicons"),
	(seti, no_seti, "seti", "Seti UI"),
	(weather, no_weather, "weather", "Weather Icons"),
);

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn positive_and_negative_switches_are_independent() {
		let flags = CategoryFlags { cod: true, no_fa: true, ..CategoryFlags::default() };

		assert_eq!(flags.included(), ["cod"]);
		assert_eq!(flags.excluded(), ["fa"]);
	}
}
