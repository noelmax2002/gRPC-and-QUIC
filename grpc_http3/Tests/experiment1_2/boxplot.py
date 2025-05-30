import os
import re
import matplotlib.pyplot as plt

# Function to read the file and extract the time elapsed data (y values)
def extract_data(file_name):
    y_vals = []
    with open(file_name, 'r') as file:
        for line in file:
            match = re.match(r"Time elapsed: \((\d+\.\d+) ,(\d+\.\d+)\)", line)
            if match:
                value = float(match.group(2))
                value *= 1000  # Convert to milliseconds
                y_vals.append(value)  # Extract y value
    return y_vals

# Function to collect all y values from a folder
def collect_folder_data(folder_path):
    all_y = []
    for file_name in os.listdir(folder_path):
        full_path = os.path.join(folder_path, file_name)
        if os.path.isfile(full_path):
            y_vals = extract_data(full_path)
            all_y.extend(y_vals)
    return all_y

# Load data
folder1 = 'quic_no_loss' 
folder2 = 'tcp_no_loss'  
folder3 = 'quic_no_loss_big' 
folder4 = 'tcp_no_loss_big'  

data1 = collect_folder_data(folder1)  # QUIC small
data2 = collect_folder_data(folder2)  # TCP small
data3 = collect_folder_data(folder3)  # QUIC big
data4 = collect_folder_data(folder4)  # TCP big

# Plot setup
plt.figure(figsize=(8, 6))

# Boxplot data
box_data = [data1, data2, data3, data4]
positions = [1, 2, 3, 4]
colors = ['blue', 'red', 'blue', 'red']  # Edge colors

# Draw boxplots
boxplots = plt.boxplot(box_data, positions=positions, patch_artist=True, showfliers=False)

# Customize boxes
for patch, color in zip(boxplots['boxes'], colors):
    patch.set(facecolor='white', edgecolor=color, linewidth=2)

# Customize other elements (whiskers, caps, medians)
for i, (whisker, cap, median) in enumerate(zip(
        zip(boxplots['whiskers'][::2], boxplots['whiskers'][1::2]),
        zip(boxplots['caps'][::2], boxplots['caps'][1::2]),
        boxplots['medians'])):
    color = colors[i]
    for w in whisker:
        w.set(color=color, linewidth=1.5)
    for c in cap:
        c.set(color=color, linewidth=1.5)
    median.set(color=color, linewidth=2)

# Set x-axis labels
plt.xticks([1.5, 3.5], ['6 kB file exchange', '60 kB file exchange'])
plt.ylabel('Time for one request [ms]')
plt.grid(True, axis='y')
plt.tight_layout()

# Custom legend
custom_legend = [
    plt.Line2D([0], [0], color='blue', lw=2, label='QUIC'),
    plt.Line2D([0], [0], color='red', lw=2, label='TCP')
]
plt.legend(handles=custom_legend, loc='upper right')

# Save and show plot
plt.savefig('Boxplot.pdf')
plt.show()
