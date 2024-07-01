# ![TockOS](http://www.tockos.org/assets/img/tock.svg "TockOS Logo")

[![tock-ci](https://github.com/tock/tock/workflows/tock-ci/badge.svg)][tock-ci]
[![slack](https://img.shields.io/badge/slack-tockos-informational)][slack]
[![book](https://img.shields.io/badge/book-Tock_Book-green)][tock-book]

Tock is an embedded operating system designed for running multiple concurrent,
mutually distrustful applications on Cortex-M and RISC-V based embedded
platforms. Tock's design centers around protection, both from potentially
malicious applications and from device drivers. Tock uses two mechanisms to
protect different components of the operating system. First, the kernel and
device drivers are written in Rust, a systems programming language that provides
compile-time memory safety and type safety. Tock uses Rust to protect the kernel
(e.g. the scheduler and hardware abstraction layer) from platform specific
device drivers as well as isolate device drivers from each other. Second, Tock
uses memory protection units to isolate applications from each other and the
kernel.

[tock-ci]: https://github.com/tock/tock/actions?query=branch%3Amaster+workflow%3Atock-ci

Tock 2.x!
---------

Tock is now on its second major release! Tock 2.x includes significant changes
from Tock 1.x, including:

- Revamped system call interface.
- Support for 11 new hardware platforms.
- Updated kernel types.
- Many new and improved HILs.

For a summary of the latest new features and improvements, check out the
[changelog](CHANGELOG.md).


Learn More
----------

How would you like to get started?

### Learn How Tock Works

Tock is documented in the [Tock Book][tock-book]. Read through the guides there
to learn about the overview and design of Tock, its implementation, and much
more.


### Use Tock

Follow our [getting started guide](doc/Getting_Started.md) to set up your system
to compile Tock.

Head to the [hardware page](https://www.tockos.org/hardware/) to learn about the
hardware platforms Tock supports. Also check out the [Tock
Book](https://book.tockos.org) for a step-by-step introduction to getting Tock
up and running.

A book on how to use Tock with the [micro:bit v2](boards/microbit_v2) and
[Raspberry Pi Pico](boards/raspberry_pi_pico) boards is [Getting Started with
Secure Embedded
Systems](https://link.springer.com/book/10.1007/978-1-4842-7789-8).

Find example applications that run on top of the Tock kernel written in both
[Rust](https://github.com/tock/libtock-rs) and
[C](https://github.com/tock/libtock-c).


### Develop Tock

Read our [getting started guide](doc/Getting_Started.md) to get the correct
version of the Rust compiler, then look through the `/kernel`, `/capsules`,
`/chips`, and `/boards` directories. There are also generated [source code
docs](https://docs.tockos.org).

We encourage contributions back to Tock and are happy to accept pull requests
for anything from small documentation fixes to whole new platforms. For details,
check out our [Contributing Guide](.github/CONTRIBUTING.md). To get started,
please do not hesitate to submit a PR. We'll happily guide you through any
needed changes.


### Keep Up To Date

Check out the [blog](https://www.tockos.org/blog/) where the **Talking Tock**
post series highlights what's new in Tock. Also, follow
[@talkingtock](https://twitter.com/talkingtock) on Twitter.

You can also browse our [email
group](https://lists.tockos.org) and our [Slack][slack]
to see discussions on Tock development.

[slack]: https://join.slack.com/t/tockos/shared_invite/enQtNDE5ODQyNDU4NTE1LWVjNTgzMTMwYzA1NDI1MjExZjljMjFmOTMxMGIwOGJlMjk0ZTI4YzY0NTYzNWM0ZmJmZGFjYmY5MTJiMDBlOTk

[tock-book]: https://book.tockos.org

Code of Conduct
---------------

The Tock project adheres to the Rust [Code of Conduct][coc].

All contributors, community members, and visitors are expected to familiarize
themselves with the Code of Conduct and to follow these standards in all
Tock-affiliated environments, which includes but is not limited to repositories,
chats, and meetup events. For moderation issues, please contact members of the
@tock/core-wg.

[coc]: https://www.rust-lang.org/conduct.html


Cite this Project
-----------------

<h4>Tock was presented at SOSP'17</h4>

Amit Levy, Bradford Campbell, Branden Ghena, Daniel B. Giffin, Pat Pannuto, Prabal Dutta, and Philip Levis. 2017. Multiprogramming a 64kB Computer Safely and Efficiently. In Proceedings of the 26th Symposium on Operating Systems Principles (SOSP ’17). Association for Computing Machinery, New York, NY, USA, 234–251. DOI: https://doi.org/10.1145/3132747.3132786

<p>
<details>
<summary>Bibtex</summary>
<pre>
@inproceedings{levy17multiprogramming,
      title = {Multiprogramming a 64kB Computer Safely and Efficiently},
      booktitle = {Proceedings of the 26th Symposium on Operating Systems Principles},
      series = {SOSP'17},
      year = {2017},
      month = {10},
      isbn = {978-1-4503-5085-3},
      location = {Shanghai, China},
      pages = {234--251},
      numpages = {18},
      url = {http://doi.acm.org/10.1145/3132747.3132786},
      doi = {10.1145/3132747.3132786},
      acmid = {3132786},
      publisher = {ACM},
      address = {New York, NY, USA},
      conference-url = {https://www.sigops.org/sosp/sosp17/},
      author = {Levy, Amit and Campbell, Bradford and Ghena, Branden and Giffin, Daniel B. and Pannuto, Pat and Dutta, Prabal and Levis, Philip},
}
</pre>
</details>
</p>


<p>This is the primary paper that describes the design considerations of Tock.</p>

<details>
  <summary>Other Tock-related papers</summary>

  <p>There are two shorter papers that look at potential limitations of the Rust language for embedded software development. The earlier PLOS paper lays out challenges and the later APSys paper lays out potential solutions. Some persons describing work on programming languages and type theory may benefit from these references, but generally, most work should cite the SOSP paper above.</p>
  <h4><a href="http://doi.acm.org/10.1145/3124680.3124717">APSys: The Case for Writing a Kernel in Rust</a></h4>
<pre>
@inproceedings{levy17rustkernel,
	title = {The Case for Writing a Kernel in Rust},
	booktitle = {Proceedings of the 8th Asia-Pacific Workshop on Systems},
	series = {APSys '17},
	year = {2017},
	month = {9},
	isbn = {978-1-4503-5197-3},
	location = {Mumbai, India},
	pages = {1:1--1:7},
	articleno = {1},
	numpages = {7},
	url = {http://doi.acm.org/10.1145/3124680.3124717},
	doi = {10.1145/3124680.3124717},
	acmid = {3124717},
	publisher = {ACM},
	address = {New York, NY, USA},
	conference-url = {https://www.cse.iitb.ac.in/~apsys2017/},
	author = {Levy, Amit and Campbell, Bradford and Ghena, Branden and Pannuto, Pat and Dutta, Prabal and Levis, Philip},
}</pre>

  <h4><a href="http://dx.doi.org/10.1145/2818302.2818306">PLOS: Ownership is Theft: Experiences Building an Embedded OS in Rust</a></h4>
<pre>
@inproceedings{levy15ownership,
	title = {Ownership is Theft: Experiences Building an Embedded {OS} in {R}ust},
	booktitle = {Proceedings of the 8th Workshop on Programming Languages and Operating Systems},
	series = {PLOS 2015},
	year = {2015},
	month = {10},
	isbn = {978-1-4503-3942-1},
	doi = {10.1145/2818302.2818306},
	url = {http://dx.doi.org/10.1145/2818302.2818306},
	location = {Monterey, CA},
	publisher = {ACM},
	address = {New York, NY, USA},
	conference-url = {http://plosworkshop.org/2015/},
	author = {Levy, Amit and Andersen, Michael P and Campbell, Bradford and Culler, David and Dutta, Prabal and Ghena, Branden and Levis, Philip and Pannuto, Pat},
}</pre>
  <p>There is also a paper on the Tock security model. The threat model documentation in the docs/ folder is the source of truth for the current Tock threat model, but this paper represents a snapshot of the reasoning behind the Tock threat model and details how it compares to those in similar embedded OSes.</p>
  <h4><a href="https://dx.doi.org/10.1145/3517208.3523752">EuroSec: Tiered Trust for useful embedded systems security</a></h4>
<pre>
@inproceedings{10.1145/3517208.3523752,
	author = {Ayers, Hudson and Dutta, Prabal and Levis, Philip and Levy, Amit and Pannuto, Pat and Van Why, Johnathan and Watson, Jean-Luc},
	title = {Tiered Trust for Useful Embedded Systems Security},
	year = {2022},
	isbn = {9781450392556},
	publisher = {Association for Computing Machinery},
	address = {New York, NY, USA},
	url = {https://doi.org/10.1145/3517208.3523752},
	doi = {10.1145/3517208.3523752},
	booktitle = {Proceedings of the 15th European Workshop on Systems Security},
	pages = {15–21},
	numpages = {7},
	keywords = {security, embedded systems, operating systems, IoT},
	location = {Rennes, France},
	series = {EuroSec '22}
}</pre>
</details>


License
-------

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
